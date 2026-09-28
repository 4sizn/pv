import { describe, expect, test } from "bun:test";
import {
  type NativeDataBatch,
  NativeDataClient,
  type NativeDataError,
  type NativeDataEvent,
  type NativeDataJoinOptions,
  type NativeDataSendResult,
  type NativeDataSnapshot,
  type NativeDataState,
  type NativeDataTransportPort,
} from "../src/native/index.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function settle() {
  for (let index = 0; index < 24; index++) await Promise.resolve();
}

function snapshot(
  revision: number,
  state: NativeDataState = "joined",
  peers: readonly string[] = ["remote"],
): NativeDataSnapshot {
  return {
    revision,
    state,
    peerId: state === "idle" || state === "destroyed" ? null : "local",
    peers: state === "idle" || state === "destroyed" ? [] : peers,
    readyPeers: [],
  };
}

const options: NativeDataJoinOptions = {
  signalingUrl: "ws://localhost:8787/ws",
  roomId: "room",
  roomToken: "test-invite",
  deviceToken: "test-device",
};

class FakeTransport implements NativeDataTransportPort {
  initial = snapshot(0, "idle");
  opening?: ReturnType<typeof deferred<NativeDataSnapshot>>;
  joining = deferred<NativeDataSnapshot>();
  leaving = deferred<NativeDataSnapshot>();
  destruction?: ReturnType<typeof deferred<NativeDataSnapshot>>;
  reads: ReturnType<typeof deferred<NativeDataBatch>>[] = [];
  readCount = 0;
  activeReads = 0;
  maximumReads = 0;
  destroys = 0;
  joins: NativeDataJoinOptions[] = [];
  sent: string[] = [];
  sendResult: NativeDataSendResult = { acceptedPeerIds: ["remote"], failures: [] };

  async open() {
    return this.opening ? this.opening.promise : this.initial;
  }
  readBatch() {
    const read = deferred<NativeDataBatch>();
    this.reads.push(read);
    this.readCount++;
    this.activeReads++;
    this.maximumReads = Math.max(this.maximumReads, this.activeReads);
    return read.promise.finally(() => this.activeReads--);
  }
  join(value: NativeDataJoinOptions) {
    this.joins.push(value);
    return this.joining.promise;
  }
  async send(data: string) {
    this.sent.push(data);
    return this.sendResult;
  }
  leave() {
    return this.leaving.promise;
  }
  async destroy() {
    this.destroys++;
    return this.destruction ? this.destruction.promise : snapshot(1000, "destroyed");
  }
  emit(value: NativeDataSnapshot, events: readonly NativeDataEvent[] = []) {
    const read = this.reads.shift();
    if (!read) throw new Error("No event read is pending");
    read.resolve({ snapshot: value, events });
  }
}

describe("native data facade", () => {
  test("uses only authoritative snapshots and projects readonly streams", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const peers: (readonly string[])[] = [];
    client.state$.subscribe((state) => states.push(state));
    client.peers$.subscribe((value) => peers.push(value));
    expect("next" in client.state$).toBe(false);
    expect("next" in client.peers$).toBe(false);
    expect("next" in client.readyPeers$).toBe(false);
    expect("next" in client.messages$).toBe(false);
    expect("next" in client.errors$).toBe(false);
    const joined = client.join(options);
    await settle();
    expect(states).toEqual(["idle"]);
    transport.joining.resolve(snapshot(2));
    await joined;
    expect(states).toEqual(["idle", "joined"]);
    expect(peers).toEqual([[], ["remote"]]);
    expect(transport.joins).toEqual([options]);
    const leaving = client.leave();
    await settle();
    expect(states).toEqual(["idle", "joined"]);
    transport.leaving.resolve(snapshot(4, "idle"));
    await leaving;
    await client.destroy();
    expect(states).toEqual(["idle", "joined", "idle", "destroyed"]);
  });

  test("runs one read at a time and emits each batch snapshot before its ordered events", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const observed: string[] = [];
    client.state$.subscribe((state) => observed.push(state));
    client.messages$.subscribe((message) => observed.push(message.data));
    client.errors$.subscribe((error) => observed.push(error.code));
    transport.emit(snapshot(1), [
      { type: "message", peerId: "remote", data: "first" },
      { type: "error", error: { code: "recoverable", message: "Peer is backpressured" } },
      { type: "message", peerId: "remote", data: "second" },
    ]);
    await settle();
    transport.emit(snapshot(1), [{ type: "message", peerId: "remote", data: "third" }]);
    await settle();
    expect(observed).toEqual(["idle", "joined", "first", "recoverable", "second", "third"]);
    expect(transport.readCount).toBe(3);
    expect(transport.maximumReads).toBe(1);
    await client.destroy();
  });

  test("membership does not imply a ready channel and readiness comes only from native snapshots", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const ready: (readonly string[])[] = [];
    client.readyPeers$.subscribe((value) => ready.push(value));
    transport.emit(snapshot(1));
    await settle();
    expect(ready).toEqual([[]]);
    transport.emit({ ...snapshot(2), readyPeers: ["remote"] });
    await settle();
    expect(ready).toEqual([[], ["remote"]]);
    transport.emit(snapshot(3));
    await settle();
    expect(ready).toEqual([[], ["remote"], []]);
    await client.destroy();
  });

  test("older command completions and batches cannot restore a departed session", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const messages: string[] = [];
    client.state$.subscribe((state) => states.push(state));
    client.messages$.subscribe((message) => messages.push(message.data));
    const joined = client.join(options);
    transport.emit(snapshot(4, "idle"));
    await settle();
    transport.joining.resolve(snapshot(2));
    await joined;
    transport.emit(snapshot(3), [{ type: "message", peerId: "remote", data: "old" }]);
    await settle();
    expect(states).toEqual(["idle"]);
    expect(messages).toEqual([]);
    await client.destroy();
  });

  test("copies and freezes transport snapshots rather than trusting caller-owned arrays", async () => {
    const transport = new FakeTransport();
    const initialPeers = ["remote"];
    transport.initial = snapshot(1, "joined", initialPeers);
    const client = await NativeDataClient.create({ transport });
    let peers: readonly string[] = [];
    client.peers$.subscribe((value) => {
      peers = value;
    });
    initialPeers.push("injected");
    expect(peers).toEqual(["remote"]);
    expect(Object.isFrozen(peers)).toBe(true);
    await client.destroy();
  });

  test("destroy interrupts pending join and read, ignores late output and completes every stream", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const messages: string[] = [];
    let completed = 0;
    client.state$.subscribe({ next: (state) => states.push(state), complete: () => completed++ });
    client.peers$.subscribe({ complete: () => completed++ });
    client.readyPeers$.subscribe({ complete: () => completed++ });
    client.messages$.subscribe({
      next: (message) => messages.push(message.data),
      complete: () => completed++,
    });
    client.errors$.subscribe({ complete: () => completed++ });
    const joined = client.join(options);
    const rejected = joined.catch((error: unknown) => error);
    const closing = client.destroy();
    expect(client.destroy()).toBe(closing);
    await closing;
    expect(await rejected).toBeInstanceOf(Error);
    expect(String(await rejected)).toContain("destroyed");
    transport.joining.resolve(snapshot(2));
    transport.emit(snapshot(2000), [{ type: "message", peerId: "remote", data: "late" }]);
    await settle();
    expect(states).toEqual(["idle", "destroyed"]);
    expect(messages).toEqual([]);
    expect(completed).toBe(5);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(1);
    await expect(client.join(options)).rejects.toThrow("destroyed");
    await expect(client.leave()).rejects.toThrow("destroyed");
    await expect(client.send("after close")).rejects.toThrow("destroyed");
    expect(transport.sent).toEqual([]);
  });

  test("reentrant snapshot teardown prevents the rest of a batch from escaping", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const messages: string[] = [];
    let closing: Promise<void> | undefined;
    client.state$.subscribe((state) => {
      if (state === "joined") closing = client.destroy();
    });
    client.messages$.subscribe((message) => messages.push(message.data));
    transport.emit(snapshot(1), [{ type: "message", peerId: "remote", data: "discard" }]);
    await settle();
    await closing;
    expect(messages).toEqual([]);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(1);
  });

  test("destroy between transport resolution and command continuation rejects the late result", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const joined = client.join(options).catch((error: unknown) => error);
    transport.joining.resolve(snapshot(2));
    const closing = Promise.resolve().then(() => client.destroy());
    expect(String(await joined)).toContain("destroyed");
    await closing;
    expect(transport.destroys).toBe(1);
  });

  test("message observers may destroy reentrantly without dispatching remaining messages", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const messages: string[] = [];
    let closing: Promise<void> | undefined;
    client.messages$.subscribe((message) => {
      messages.push(message.data);
      closing = client.destroy();
    });
    transport.emit(snapshot(1), [
      { type: "message", peerId: "remote", data: "first" },
      { type: "message", peerId: "remote", data: "second" },
    ]);
    await settle();
    await closing;
    expect(messages).toEqual(["first"]);
    expect(transport.destroys).toBe(1);
  });

  test("failed native cleanup remains terminal without inventing a destroyed snapshot", async () => {
    const transport = new FakeTransport();
    transport.destruction = deferred<NativeDataSnapshot>();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const errors: NativeDataError[] = [];
    let completed = 0;
    client.state$.subscribe({ next: (state) => states.push(state), complete: () => completed++ });
    client.peers$.subscribe({ complete: () => completed++ });
    client.readyPeers$.subscribe({ complete: () => completed++ });
    client.messages$.subscribe({ complete: () => completed++ });
    client.errors$.subscribe({ next: (error) => errors.push(error), complete: () => completed++ });
    const closing = client.destroy();
    const rejected = closing.catch((error: unknown) => error);
    transport.destruction.reject(new Error("cleanup failed"));
    expect(String(await rejected)).toContain("cleanup failed");
    expect(client.destroy()).toBe(closing);
    expect(states).toEqual(["idle"]);
    expect(errors.map((error) => error.code)).toEqual(["cleanup-failed"]);
    expect(completed).toBe(5);
    await expect(client.send("late")).rejects.toThrow("destroyed");
    transport.reads[0]?.reject(new Error("late read failure"));
    await settle();
    expect(transport.readCount).toBe(1);
  });

  test("malformed initialization releases the native owner before rejecting", async () => {
    const transport = new FakeTransport();
    transport.initial = { ...snapshot(0, "idle"), state: "unexpected" as NativeDataState };
    await expect(NativeDataClient.create({ transport })).rejects.toThrow("Invalid native");
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(0);
  });

  test("initialization failure reports both the open and cleanup failures", async () => {
    const transport = new FakeTransport();
    transport.opening = deferred<NativeDataSnapshot>();
    transport.destruction = deferred<NativeDataSnapshot>();
    const creating = NativeDataClient.create({ transport });
    const rejected = creating.catch((error: unknown) => error);
    transport.opening.reject(new Error("open failed"));
    await settle();
    transport.destruction.reject(new Error("cleanup failed"));
    expect(await rejected).toBeInstanceOf(AggregateError);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(0);
  });

  test("a changed snapshot at the same revision rejects the feed and releases native resources", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const errors: NativeDataError[] = [];
    client.state$.subscribe((state) => states.push(state));
    client.errors$.subscribe((error) => errors.push(error));
    transport.emit(snapshot(0));
    await settle();
    expect(states).toEqual(["idle", "destroyed"]);
    expect(errors.map((error) => error.code)).toEqual(["transport-failed"]);
    expect(transport.destroys).toBe(1);
  });

  test("validates an entire event batch before exposing any of it", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const messages: string[] = [];
    client.messages$.subscribe((message) => messages.push(message.data));
    transport.emit(snapshot(1), [
      { type: "message", peerId: "remote", data: "must not escape" },
      { type: "message", peerId: "remote", data: "😀".repeat(4097) },
    ]);
    await settle();
    expect(messages).toEqual([]);
    expect(transport.destroys).toBe(1);
  });

  test("caps event batches and rejects impossible membership snapshots", async () => {
    for (const invalid of [
      {
        snapshot: snapshot(1),
        events: Array(33).fill({ type: "message", peerId: "a", data: "x" }),
      },
      { snapshot: snapshot(Number.MAX_SAFE_INTEGER + 1), events: [] },
      { snapshot: snapshot(1, "joined", ["same", "same"]), events: [] },
      { snapshot: snapshot(1, "joined", ["local"]), events: [] },
      { snapshot: { ...snapshot(1), peerId: null }, events: [] },
      { snapshot: { ...snapshot(1), readyPeers: ["not-a-member"] }, events: [] },
    ]) {
      const transport = new FakeTransport();
      const client = await NativeDataClient.create({ transport });
      let failed = false;
      client.errors$.subscribe(() => {
        failed = true;
      });
      transport.reads.shift()?.resolve(invalid);
      await settle();
      expect(failed).toBe(true);
      expect(transport.destroys).toBe(1);
    }
  });

  test("returns per-peer send acceptance and enforces bytes rather than UTF-16 length", async () => {
    const transport = new FakeTransport();
    transport.sendResult = {
      acceptedPeerIds: ["first"],
      failures: [{ peerId: "second", message: "Peer is backpressured" }],
    };
    const client = await NativeDataClient.create({ transport });
    const data = "😀".repeat(4096);
    const result = await client.send(data);
    expect(result).toEqual(transport.sendResult);
    expect(Object.isFrozen(result.failures[0])).toBe(true);
    await expect(client.send(`${data}a`)).rejects.toThrow("UTF-8 bytes");
    expect(transport.sent).toEqual([data]);
    transport.sendResult = {
      acceptedPeerIds: ["same"],
      failures: [{ peerId: "same", message: "x" }],
    };
    await expect(client.send("invalid receipt")).rejects.toThrow("Invalid native");
    await client.destroy();
  });

  test("read failure releases the owner and delivers an error value before stream completion", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const order: string[] = [];
    client.errors$.subscribe({
      next: (error) => order.push(error.code),
      complete: () => order.push("complete"),
    });
    transport.reads.shift()?.reject(new Error("socket no longer exists"));
    await settle();
    expect(order).toEqual(["transport-failed", "complete"]);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(1);
  });

  test("a stale destroy response cannot be reported as successful cleanup", async () => {
    const transport = new FakeTransport();
    transport.initial = snapshot(2000);
    const client = await NativeDataClient.create({ transport });
    let completed = false;
    client.state$.subscribe({
      complete: () => {
        completed = true;
      },
    });
    await expect(client.destroy()).rejects.toThrow("Invalid native");
    expect(completed).toBe(true);
    await expect(client.join(options)).rejects.toThrow("destroyed");
  });

  test("a native terminal snapshot closes the facade and detaches its event pump", async () => {
    const transport = new FakeTransport();
    const client = await NativeDataClient.create({ transport });
    const states: NativeDataState[] = [];
    const messages: string[] = [];
    let completed = false;
    client.state$.subscribe({
      next: (state) => states.push(state),
      complete: () => {
        completed = true;
      },
    });
    client.messages$.subscribe((message) => messages.push(message.data));
    transport.emit(snapshot(3, "destroyed"), [{ type: "message", peerId: "remote", data: "old" }]);
    await settle();
    expect(states).toEqual(["idle", "destroyed"]);
    expect(completed).toBe(true);
    expect(messages).toEqual([]);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(1);
  });
});
