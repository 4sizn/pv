import { afterEach, describe, expect, test } from "bun:test";
import {
  BrowserSignalingAdapter,
  captureCamera,
  fromBrowserTrack,
  toBrowserTrack,
} from "../src/browser/index.js";
import type { SignalingEvent } from "../src/index.js";

class FakeSocket {
  static OPEN = 1;
  static instances: FakeSocket[] = [];
  readonly sent: string[] = [];
  readyState = 0;
  bufferedAmount = 0;
  closes = 0;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.instances.push(this);
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.closes++;
    this.readyState = 3;
    this.onclose?.();
  }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  receive(value: unknown) {
    this.onmessage?.({ data: JSON.stringify(value) });
  }
}

const originalSocket = globalThis.WebSocket;
const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
afterEach(() => {
  globalThis.WebSocket = originalSocket;
  if (originalNavigator) Object.defineProperty(globalThis, "navigator", originalNavigator);
  else Reflect.deleteProperty(globalThis, "navigator");
  FakeSocket.instances = [];
});

const credentials = { roomId: "room", roomToken: "invitation", deviceToken: "credential" };
function installSocket() {
  globalThis.WebSocket = FakeSocket as unknown as typeof WebSocket;
}

describe("browser signaling operation cancellation", () => {
  test("cancelled socket cannot send late join or deliver stale events during reuse", async () => {
    installSocket();
    const adapter = new BrowserSignalingAdapter("ws://localhost/ws");
    const events: SignalingEvent[] = [];
    adapter.events$.subscribe((event) => events.push(event));
    const first = adapter.connect(credentials).catch((error: Error) => error);
    const old = FakeSocket.instances[0] as FakeSocket;
    const oldOpen = old.onopen;
    const oldMessage = old.onmessage;
    adapter.close();
    expect(((await first) as Error).message).toContain("cancelled");
    const second = adapter.connect(credentials);
    oldOpen?.();
    oldMessage?.({ data: JSON.stringify({ type: "peer-joined", peerId: "stale" }) });
    expect(old.sent).toEqual([]);
    expect(events).toEqual([]);
    const current = FakeSocket.instances[1] as FakeSocket;
    current.open();
    expect(JSON.parse(current.sent[0] ?? "null")).toEqual({ type: "join", ...credentials });
    current.receive({ type: "joined", peerId: "a", peers: [] });
    await expect(second).resolves.toEqual({ peerId: "a", peers: [] });
    adapter.close();
    adapter.close();
    expect(current.closes).toBe(1);
    expect(old.closes).toBe(1);
  });

  test("wire validation emits recoverable errors without terminating the event stream", async () => {
    installSocket();
    const adapter = new BrowserSignalingAdapter("ws://localhost/ws");
    const events: SignalingEvent[] = [];
    adapter.events$.subscribe((event) => events.push(event));
    const joined = adapter.connect(credentials);
    const socket = FakeSocket.instances[0] as FakeSocket;
    socket.open();
    socket.receive({ type: "joined", peerId: "a", peers: ["b"] });
    await joined;
    socket.receive({
      type: "signal",
      from: "b",
      payload: { type: "description", description: { type: "invalid", sdp: 42 } },
    });
    socket.receive({ type: "peer-left", peerId: "b" });
    expect(events.map((event) => event.type)).toEqual(["error", "peer-left"]);
    adapter.ping();
    expect(JSON.parse(socket.sent.at(-1) ?? "null")).toEqual({ type: "ping" });
    adapter.close();
  });
});

describe("browser capture ownership", () => {
  test("permission result arriving after cancellation stops every acquired track", async () => {
    let finish!: (stream: MediaStream) => void;
    const pending = new Promise<MediaStream>((resolve) => {
      finish = resolve;
    });
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: { mediaDevices: { getUserMedia: () => pending } },
    });
    const abort = new AbortController();
    let stops = 0;
    const native = {
      id: "late",
      kind: "video",
      stop: () => {
        stops++;
      },
    } as unknown as MediaStreamTrack;
    const capturing = captureCamera({ signal: abort.signal }).catch((error: Error) => error);
    abort.abort(new Error("capture cancelled"));
    finish({ getTracks: () => [native] } as unknown as MediaStream);
    expect(((await capturing) as Error).message).toBe("capture cancelled");
    expect(stops).toBe(1);
  });

  test("browser handles preserve the native playback track without core casts", () => {
    const native = { id: "camera", kind: "video", stop: () => {} } as unknown as MediaStreamTrack;
    expect(toBrowserTrack(fromBrowserTrack(native))).toBe(native);
    expect(() => toBrowserTrack({ id: "foreign", kind: "video", stop: () => {} })).toThrow(
      "browser adapter",
    );
  });
});
