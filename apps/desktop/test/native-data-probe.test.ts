import { describe, expect, test } from "bun:test";
import type {
  NativeDataError,
  NativeDataJoinOptions,
  NativeDataMessage,
  NativeDataSendResult,
} from "@parentview/media-sdk/native";
import { BehaviorSubject, Subject } from "rxjs";
import type { RoomInvitation } from "../src/invitation";
import type { DeviceIdentity } from "../src/laboratory-api";
import {
  NativeDataProbe,
  type NativeProbeApi,
  type NativeProbeClient,
  type NativeProbeDependencies,
  type NativeProbeStatus,
  type NativeProbeView,
} from "../src/native-data-probe";

const room: RoomInvitation = {
  version: 1,
  signalingOrigin: "http://localhost:8787",
  roomId: "test-room",
  roomToken: "test-only-room-secret",
  iceServers: [
    { urls: "turn:relay.example.test", username: "fixture", credential: "test-only-ice" },
  ],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function until(predicate: () => boolean): Promise<void> {
  for (let index = 0; index < 100; index += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error("Expected workflow checkpoint was not reached");
}

class FakeView implements NativeProbeView {
  origin = "http://localhost:8787/";
  action = () => {};
  updates: { status: NativeProbeStatus; message: string }[] = [];
  bindNativeProbe(action: () => void): void {
    this.action = action;
  }
  setNativeProbe(status: NativeProbeStatus, message: string): void {
    this.updates.push({ status, message });
  }
  get status(): NativeProbeStatus | undefined {
    return this.updates.at(-1)?.status;
  }
}

class FakeApi implements NativeProbeApi {
  clears = 0;
  registrations: { origin: string; signal: AbortSignal }[] = [];
  rooms: DeviceIdentity[] = [];
  constructor(readonly peerId: string) {}
  async identity(origin: string, signal: AbortSignal): Promise<DeviceIdentity> {
    this.registrations.push({ origin, signal });
    return { peerId: this.peerId, deviceToken: `test-only-device-${this.peerId}` };
  }
  async createRoom(_origin: string, identity: DeviceIdentity): Promise<RoomInvitation> {
    this.rooms.push(identity);
    return room;
  }
  clear(): void {
    this.clears += 1;
  }
}

class FakeClient implements NativeProbeClient {
  readonly ready = new BehaviorSubject<readonly string[]>([]);
  readonly messages = new Subject<NativeDataMessage>();
  readonly errors = new Subject<NativeDataError>();
  readonly readyPeers$ = this.ready.asObservable();
  readonly messages$ = this.messages.asObservable();
  readonly errors$ = this.errors.asObservable();
  joins: NativeDataJoinOptions[] = [];
  sent: string[] = [];
  leaves = 0;
  destroys = 0;
  onJoin: () => Promise<void> = async () => {};
  onSend: (data: string) => Promise<NativeDataSendResult> = async () => ({
    acceptedPeerIds: [],
    failures: [],
  });
  onLeave: () => Promise<void> = async () => {};
  onDestroy: () => Promise<void> = async () => {};
  async join(options: NativeDataJoinOptions): Promise<void> {
    this.joins.push(options);
    await this.onJoin();
  }
  async send(data: string): Promise<NativeDataSendResult> {
    this.sent.push(data);
    return this.onSend(data);
  }
  async leave(): Promise<void> {
    this.leaves += 1;
    await this.onLeave();
  }
  async destroy(): Promise<void> {
    this.destroys += 1;
    await this.onDestroy();
  }
}

function setup(timeoutMs = 40) {
  const view = new FakeView();
  const clients = [new FakeClient(), new FakeClient()];
  const apis = [new FakeApi("peer-a"), new FakeApi("peer-b")];
  let createdClients = 0;
  let createdApis = 0;
  const dependencies: NativeProbeDependencies = {
    timeoutMs,
    createApi: () => apis[createdApis++],
    createClient: async () => clients[createdClients++],
  };
  for (const [index, client] of clients.entries()) {
    client.onJoin = async () => {
      if (clients.every((peer) => peer.joins.length > 0)) {
        clients[0].ready.next([apis[1].peerId]);
        clients[1].ready.next([apis[0].peerId]);
      }
    };
    client.onSend = async (data) => {
      clients[1 - index].messages.next({ peerId: apis[index].peerId, data });
      return { acceptedPeerIds: [apis[1 - index].peerId], failures: [] };
    };
  }
  const probe = new NativeDataProbe(view, dependencies);
  return { probe, view, clients, apis, dependencies, createdClients: () => createdClients };
}

function expectReleased(clients: FakeClient[], apis: FakeApi[]): void {
  for (const client of clients) {
    expect(client.leaves).toBe(1);
    expect(client.destroys).toBe(1);
    expect(client.ready.observers).toHaveLength(0);
    expect(client.messages.observers).toHaveLength(0);
    expect(client.errors.observers).toHaveLength(0);
  }
  for (const api of apis) expect(api.clears).toBe(1);
}

describe("native developer data proof", () => {
  test("uses two authenticated identities, returned ICE and exact bidirectional messages before cleanup-gated success", async () => {
    const { probe, view, clients, apis } = setup();
    const destroyGate = deferred<void>();
    clients[0].onDestroy = () => destroyGate.promise;
    const run = probe.run();
    await until(() => clients[0].destroys === 1);
    expect(view.status).toBe("running");
    expect(clients.map((client) => client.sent.length)).toEqual([1, 1]);
    expect(clients[0].sent[0]).not.toBe(clients[1].sent[0]);
    for (const [index, client] of clients.entries()) {
      expect(client.joins).toEqual([
        {
          signalingUrl: "ws://localhost:8787/ws",
          roomId: room.roomId,
          roomToken: room.roomToken,
          deviceToken: `test-only-device-${apis[index].peerId}`,
          iceServers: [{ ...room.iceServers[0], urls: ["turn:relay.example.test"] }],
        },
      ]);
      expect(apis[index].registrations[0].origin).toBe("http://localhost:8787");
    }
    expect(apis.map((api) => api.rooms.length)).toEqual([1, 0]);
    destroyGate.resolve();
    await run;
    expect(view.status).toBe("success");
    expectReleased(clients, apis);
    expect(JSON.stringify(view.updates)).not.toContain("test-only-");
  });

  test("requires the expected ready peer before sending, not a different ready peer", async () => {
    const { probe, view, clients, apis } = setup();
    for (const client of clients) client.onJoin = async () => client.ready.next(["unrelated-peer"]);
    await probe.run();
    expect(view.status).toBe("error");
    expect(clients.map((client) => client.sent.length)).toEqual([0, 0]);
    expectReleased(clients, apis);
  });

  test("local acceptance and forged/wrong payload events cannot substitute for remote delivery", async () => {
    const { probe, view, clients, apis } = setup();
    clients[0].onSend = async (data) => {
      clients[1].messages.next({ peerId: "unrelated-peer", data });
      clients[1].messages.next({ peerId: apis[0].peerId, data: "wrong payload" });
      return { acceptedPeerIds: [apis[1].peerId], failures: [] };
    };
    await probe.run();
    expect(view.status).toBe("error");
    expectReleased(clients, apis);
  });

  test("rejects send failures even if matching delivery events were emitted", async () => {
    const { probe, view, clients, apis } = setup();
    clients[0].onSend = async (data) => {
      clients[1].messages.next({ peerId: apis[0].peerId, data });
      return {
        acceptedPeerIds: [],
        failures: [{ peerId: apis[1].peerId, message: "test-only-secret" }],
      };
    };
    await probe.run();
    expect(view.status).toBe("error");
    expectReleased(clients, apis);
    expect(JSON.stringify(view.updates)).not.toContain("test-only-secret");
  });

  test("suppresses duplicate clicks and destroys a handle created after pagehide without joining", async () => {
    const { probe, view, clients, apis, dependencies } = setup();
    const creation = deferred<NativeProbeClient>();
    let creations = 0;
    dependencies.createClient = () => {
      creations += 1;
      return creation.promise;
    };
    const run = probe.run();
    expect(probe.run()).toBe(run);
    await until(() => creations === 1);
    const previousUpdates = view.updates.length;
    const stopped = probe.destroy();
    let finished = false;
    void stopped.then(() => {
      finished = true;
    });
    await Promise.resolve();
    expect(finished).toBe(false);
    creation.resolve(clients[0]);
    await stopped;
    await probe.destroy();
    await probe.run();
    expect(creations).toBe(1);
    expect(view.updates).toHaveLength(previousUpdates);
    expect(clients[0].joins).toHaveLength(0);
    expectReleased([clients[0]], apis);
    expect(clients[1].destroys).toBe(0);
  });

  test("pagehide interrupts a pending join and releases both clients without late UI", async () => {
    const { probe, view, clients, apis } = setup();
    const join = deferred<void>();
    clients[0].onJoin = () => join.promise;
    clients[0].onDestroy = async () => join.resolve();
    const run = probe.run();
    await until(() => clients[1].joins.length === 1);
    const previousUpdates = view.updates.length;
    await probe.destroy();
    await run;
    expect(view.updates).toHaveLength(previousUpdates);
    expectReleased(clients, apis);
  });

  test("native error events stop waiting and expose only a fixed safe failure", async () => {
    const { probe, view, clients, apis } = setup(10_000);
    for (const client of clients) client.onJoin = async () => {};
    const run = probe.run();
    await until(() => clients[0].ready.observers.length > 0);
    clients[0].errors.next({ code: "failed", message: "test-only-secret" });
    await run;
    expect(view.status).toBe("error");
    expect(JSON.stringify(view.updates)).not.toContain("test-only-secret");
    expectReleased(clients, apis);
  });

  test("leave and destroy failures never produce success; every client is still destroyed and retry is available", async () => {
    const { probe, view, clients, apis, dependencies } = setup();
    clients[0].onLeave = async () => {
      throw new Error("test-only-secret");
    };
    clients[1].onDestroy = async () => {
      throw new Error("test-only-secret");
    };
    await probe.run();
    expect(view.status).toBe("error");
    expect(view.updates.at(-1)?.message).toContain("자원 정리");
    expectReleased(clients, apis);
    const retry = setup();
    dependencies.createApi = retry.dependencies.createApi;
    dependencies.createClient = retry.dependencies.createClient;
    await probe.run();
    expect(view.status).toBe("success");
    expectReleased(retry.clients, retry.apis);
  });

  test("releases the first client if creating the second fails", async () => {
    const { probe, view, clients, apis, dependencies } = setup();
    let creations = 0;
    dependencies.createClient = async () => {
      if (creations++ === 1) throw new Error("test-only-secret");
      return clients[0];
    };
    await probe.run();
    expect(view.status).toBe("error");
    expectReleased([clients[0]], apis);
    expect(clients[0].joins).toHaveLength(0);
  });

  test("waits for late identity registration before clearing its cache after another registration fails", async () => {
    const { probe, view, apis, createdClients } = setup();
    const identity = deferred<DeviceIdentity>();
    apis[0].identity = async () => {
      throw new Error("test-only-secret");
    };
    apis[1].identity = () => identity.promise;
    const run = probe.run();
    await until(() => view.status === "running");
    expect(apis[1].clears).toBe(0);
    identity.resolve({ peerId: "late-peer", deviceToken: "test-only-late-secret" });
    await run;
    expect(view.status).toBe("error");
    expect(apis.map((api) => api.clears)).toEqual([1, 1]);
    expect(createdClients()).toBe(0);
  });

  test("rejects an invalid origin before allocating credentials or native clients", async () => {
    const { probe, view, apis, createdClients } = setup();
    view.origin = "https://user:test-only-secret@example.test";
    await probe.run();
    expect(view.status).toBe("error");
    expect(createdClients()).toBe(0);
    expect(apis.map((api) => api.registrations.length)).toEqual([0, 0]);
    expect(JSON.stringify(view.updates)).not.toContain("test-only-secret");
  });
});
