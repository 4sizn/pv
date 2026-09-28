import { describe, expect, test } from "bun:test";
import {
  type NativeDataSendResult,
  type NativeLocalSource,
  type NativeMediaBatch,
  NativeMediaClient,
  type NativeMediaError,
  type NativeMediaEvent,
  type NativeMediaJoinOptions,
  type NativeMediaSnapshot,
  type NativeMediaState,
  type NativeMediaTransportPort,
  type NativeRemoteSource,
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
  state: NativeMediaState = "joined",
  peers: readonly string[] = ["remote"],
): NativeMediaSnapshot {
  return {
    revision,
    state,
    peerId: state === "idle" || state === "destroyed" ? null : "local",
    peers: state === "idle" || state === "destroyed" ? [] : peers,
    readyPeers: [],
    localSources: [],
    remoteSources: [],
  };
}

const options: NativeMediaJoinOptions = {
  signalingUrl: "ws://localhost:8787/ws",
  roomId: "room",
  roomToken: "test-invite",
  deviceToken: "test-device",
};

class FakeTransport implements NativeMediaTransportPort {
  initial = snapshot(0, "idle");
  opening?: ReturnType<typeof deferred<NativeMediaSnapshot>>;
  joining = deferred<NativeMediaSnapshot>();
  leaving = deferred<NativeMediaSnapshot>();
  destruction?: ReturnType<typeof deferred<NativeMediaSnapshot>>;
  reads: ReturnType<typeof deferred<NativeMediaBatch>>[] = [];
  readCount = 0;
  activeReads = 0;
  maximumReads = 0;
  destroys = 0;
  joins: NativeMediaJoinOptions[] = [];
  sent: string[] = [];
  sendResult: NativeDataSendResult = { acceptedPeerIds: ["remote"], failures: [] };

  async open() {
    return this.opening ? this.opening.promise : this.initial;
  }
  readBatch() {
    const read = deferred<NativeMediaBatch>();
    this.reads.push(read);
    this.readCount++;
    this.activeReads++;
    this.maximumReads = Math.max(this.maximumReads, this.activeReads);
    return read.promise.finally(() => this.activeReads--);
  }
  join(value: NativeMediaJoinOptions) {
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
  emit(value: NativeMediaSnapshot, events: readonly NativeMediaEvent[] = []) {
    const read = this.reads.shift();
    if (!read) throw new Error("No event read is pending");
    read.resolve({ snapshot: value, events });
  }
}

describe("native media facade", () => {
  test("uses only authoritative snapshots and projects readonly streams", async () => {
    const transport = new FakeTransport();
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
    const peers: (readonly string[])[] = [];
    client.state$.subscribe((state) => states.push(state));
    client.peers$.subscribe((value) => peers.push(value));
    expect("next" in client.state$).toBe(false);
    expect("next" in client.peers$).toBe(false);
    expect("next" in client.readyPeers$).toBe(false);
    expect("next" in client.localSources$).toBe(false);
    expect("next" in client.remoteSources$).toBe(false);
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
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
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

  test("projects only native source metadata, suppresses equal values and preserves meaningful source changes", async () => {
    const transport = new FakeTransport();
    const client = await NativeMediaClient.create({ transport });
    const locals: (readonly NativeLocalSource[])[] = [];
    const remotes: (readonly NativeRemoteSource[])[] = [];
    client.localSources$.subscribe((sources) => locals.push(sources));
    client.remoteSources$.subscribe((sources) => remotes.push(sources));
    const local: NativeLocalSource = { id: "camera-local", kind: "camera" };
    const remote: NativeRemoteSource = {
      peerId: "remote",
      id: "camera-remote",
      kind: "camera",
      mid: "0",
    };
    const joining = client.join(options);
    await settle();
    expect(locals).toEqual([[]]);
    expect(remotes).toEqual([[]]);
    transport.joining.resolve({ ...snapshot(2), localSources: [local], remoteSources: [remote] });
    await joining;
    transport.emit({
      ...snapshot(3),
      localSources: [{ ...local }],
      remoteSources: [{ ...remote }],
    });
    await settle();
    expect(locals).toEqual([[], [local]]);
    expect(remotes).toEqual([[], [remote]]);
    const remapped = { ...remote, mid: "1" };
    const replacement = { id: "camera-replacement", kind: "camera" as const };
    transport.emit({ ...snapshot(4), localSources: [replacement], remoteSources: [remapped] });
    await settle();
    expect(locals).toEqual([[], [local], [replacement]]);
    expect(remotes).toEqual([[], [remote], [remapped]]);
    transport.emit({
      ...snapshot(5, "leaving"),
      localSources: [replacement],
      remoteSources: [remapped],
    });
    await settle();
    expect(locals).toHaveLength(3);
    const leaving = client.leave();
    transport.leaving.resolve(snapshot(6, "idle"));
    await leaving;
    transport.emit({ ...snapshot(4), localSources: [local], remoteSources: [remote] });
    await settle();
    expect(locals.at(-1)).toEqual([]);
    expect(remotes.at(-1)).toEqual([]);
    expect(locals).toHaveLength(4);
    expect(remotes).toHaveLength(4);
    await client.destroy();
  });

  test("copies and freezes every source descriptor and removes non-contract frame fields", async () => {
    const transport = new FakeTransport();
    const local = {
      id: "local-camera",
      kind: "camera" as const,
      frame: "must not cross the facade",
    };
    const remote = {
      id: "remote-camera",
      kind: "camera" as const,
      peerId: "remote",
      mid: "0",
      nativeHandle: "private",
    };
    transport.initial = { ...snapshot(1), localSources: [local], remoteSources: [remote] };
    const client = await NativeMediaClient.create({ transport });
    let locals: readonly NativeLocalSource[] = [];
    let remotes: readonly NativeRemoteSource[] = [];
    client.localSources$.subscribe((value) => {
      locals = value;
    });
    client.remoteSources$.subscribe((value) => {
      remotes = value;
    });
    local.id = "mutated";
    remote.mid = "mutated";
    expect(locals).toEqual([{ id: "local-camera", kind: "camera" }]);
    expect(remotes).toEqual([{ id: "remote-camera", kind: "camera", peerId: "remote", mid: "0" }]);
    for (const value of [locals, locals[0], remotes, remotes[0]])
      expect(Object.isFrozen(value)).toBe(true);
    expect("publish" in client).toBe(false);
    expect("capture" in client).toBe(false);
    await client.destroy();
  });

  test("accepts three local and nine remote sources with uniqueness scoped to each peer", async () => {
    const transport = new FakeTransport();
    const kinds = ["camera", "screen", "microphone"] as const;
    const localSources = kinds.map((kind) => ({ id: `${kind}-local`, kind }));
    const remoteSources = ["a", "b", "c"].flatMap((peerId) =>
      kinds.map((kind, index) => ({ peerId, id: kind, kind, mid: String(index) })),
    );
    transport.initial = { ...snapshot(1, "joined", ["a", "b", "c"]), localSources, remoteSources };
    const client = await NativeMediaClient.create({ transport });
    let observed: readonly NativeRemoteSource[] = [];
    client.remoteSources$.subscribe((value) => {
      observed = value;
    });
    expect(observed).toEqual(remoteSources);
    await client.destroy();
  });

  test("requires bounded source arrays, known peers, supported kinds and all uniqueness constraints", async () => {
    const local = { id: "camera", kind: "camera" };
    const remote = { ...local, peerId: "remote", mid: "0" };
    const invalid: Record<string, unknown>[] = [
      { localSources: undefined },
      { remoteSources: undefined },
      { localSources: null },
      { remoteSources: {} },
      { localSources: Array(1) },
      { remoteSources: Array(1) },
      { localSources: [null] },
      { remoteSources: ["camera"] },
      { localSources: Array(4).fill(local) },
      { remoteSources: Array(10).fill(remote) },
      { localSources: [{ ...local, kind: "video" }] },
      { remoteSources: [{ ...remote, kind: "audio" }] },
      { localSources: [local, { ...local, kind: "screen" }] },
      { localSources: [local, { ...local, id: "second-camera" }] },
      { remoteSources: [{ ...remote, peerId: "unknown" }] },
      { remoteSources: [{ ...remote, peerId: "local" }] },
      { remoteSources: [remote, { ...remote, kind: "screen", mid: "1" }] },
      { remoteSources: [remote, { ...remote, id: "second-camera", mid: "1" }] },
      { remoteSources: [remote, { ...remote, id: "screen", kind: "screen" }] },
    ];
    for (const state of ["idle", "joining", "destroyed"] as const) {
      invalid.push(
        { ...snapshot(1, state), localSources: [local] },
        { ...snapshot(1, state), remoteSources: [remote] },
      );
    }
    for (const patch of invalid) {
      const transport = new FakeTransport();
      transport.initial = { ...snapshot(1), ...patch } as NativeMediaSnapshot;
      await expect(NativeMediaClient.create({ transport })).rejects.toThrow("Invalid native");
      expect(transport.destroys).toBe(1);
      expect(transport.readCount).toBe(0);
    }
  });

  test("source ids and MIDs allow exactly 128 UTF-8 bytes and reject empty/control identifiers", async () => {
    const validId = "😀".repeat(32);
    const transport = new FakeTransport();
    transport.initial = {
      ...snapshot(1),
      localSources: [{ id: validId, kind: "camera" }],
      remoteSources: [{ id: "x".repeat(128), kind: "screen", peerId: "remote", mid: validId }],
    };
    const client = await NativeMediaClient.create({ transport });
    await client.destroy();
    for (const invalidId of [
      "",
      "x".repeat(129),
      `${validId}x`,
      "before\u0000after",
      "line\nend",
      "\u007f",
      "\u0085",
      123,
      null,
    ]) {
      for (const patch of [
        { localSources: [{ id: invalidId, kind: "camera" }] },
        { remoteSources: [{ id: invalidId, kind: "screen", peerId: "remote", mid: "0" }] },
        { remoteSources: [{ id: "screen", kind: "screen", peerId: "remote", mid: invalidId }] },
      ]) {
        const transport = new FakeTransport();
        transport.initial = { ...snapshot(1), ...patch } as NativeMediaSnapshot;
        await expect(NativeMediaClient.create({ transport })).rejects.toThrow("Invalid native");
        expect(transport.destroys).toBe(1);
      }
    }
  });

  test("source changes at the same revision invalidate the feed before events escape", async () => {
    const local = { id: "camera", kind: "camera" as const };
    const remote = { ...local, peerId: "remote", mid: "0" };
    for (const changed of [
      { localSources: [] },
      { localSources: [{ ...local, id: "replacement" }] },
      { localSources: [{ ...local, kind: "screen" as const }] },
      { remoteSources: [] },
      { remoteSources: [{ ...remote, id: "replacement" }] },
      { remoteSources: [{ ...remote, kind: "screen" as const }] },
      { remoteSources: [{ ...remote, mid: "1" }] },
      { remoteSources: [{ ...remote, peerId: "other" }] },
    ]) {
      const transport = new FakeTransport();
      transport.initial = {
        ...snapshot(1, "joined", ["remote", "other"]),
        localSources: [local],
        remoteSources: [remote],
      };
      const client = await NativeMediaClient.create({ transport });
      const messages: string[] = [];
      const errors: string[] = [];
      client.messages$.subscribe((value) => messages.push(value.data));
      client.errors$.subscribe((value) => errors.push(value.code));
      transport.emit({ ...transport.initial, ...changed }, [
        { type: "message", peerId: "remote", data: "must not escape" },
      ]);
      await settle();
      expect(messages).toEqual([]);
      expect(errors).toEqual(["transport-failed"]);
      expect(transport.destroys).toBe(1);
    }
  });

  test("malformed source metadata rejects a whole batch before its messages are visible", async () => {
    const transport = new FakeTransport();
    const client = await NativeMediaClient.create({ transport });
    const messages: string[] = [];
    const sources: (readonly NativeRemoteSource[])[] = [];
    client.messages$.subscribe((value) => messages.push(value.data));
    client.remoteSources$.subscribe((value) => sources.push(value));
    transport.emit(
      {
        ...snapshot(1),
        remoteSources: [{ peerId: "unknown", id: "camera", kind: "camera", mid: "0" }],
      },
      [{ type: "message", peerId: "remote", data: "must not escape" }],
    );
    await settle();
    expect(messages).toEqual([]);
    expect(sources).toEqual([[]]);
    expect(transport.destroys).toBe(1);
  });

  test("copies and freezes transport snapshots rather than trusting caller-owned arrays", async () => {
    const transport = new FakeTransport();
    const initialPeers = ["remote"];
    transport.initial = snapshot(1, "joined", initialPeers);
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
    const messages: string[] = [];
    let completed = 0;
    client.state$.subscribe({ next: (state) => states.push(state), complete: () => completed++ });
    client.peers$.subscribe({ complete: () => completed++ });
    client.readyPeers$.subscribe({ complete: () => completed++ });
    client.localSources$.subscribe({ complete: () => completed++ });
    client.remoteSources$.subscribe({ complete: () => completed++ });
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
    expect(completed).toBe(7);
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(1);
    await expect(client.join(options)).rejects.toThrow("destroyed");
    await expect(client.leave()).rejects.toThrow("destroyed");
    await expect(client.send("after close")).rejects.toThrow("destroyed");
    expect(transport.sent).toEqual([]);
  });

  test("reentrant snapshot teardown prevents the rest of a batch from escaping", async () => {
    const transport = new FakeTransport();
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
    const joined = client.join(options).catch((error: unknown) => error);
    transport.joining.resolve(snapshot(2));
    const closing = Promise.resolve().then(() => client.destroy());
    expect(String(await joined)).toContain("destroyed");
    await closing;
    expect(transport.destroys).toBe(1);
  });

  test("message observers may destroy reentrantly without dispatching remaining messages", async () => {
    const transport = new FakeTransport();
    const client = await NativeMediaClient.create({ transport });
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
    transport.destruction = deferred<NativeMediaSnapshot>();
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
    const errors: NativeMediaError[] = [];
    let completed = 0;
    client.state$.subscribe({ next: (state) => states.push(state), complete: () => completed++ });
    client.peers$.subscribe({ complete: () => completed++ });
    client.readyPeers$.subscribe({ complete: () => completed++ });
    client.localSources$.subscribe({ complete: () => completed++ });
    client.remoteSources$.subscribe({ complete: () => completed++ });
    client.messages$.subscribe({ complete: () => completed++ });
    client.errors$.subscribe({ next: (error) => errors.push(error), complete: () => completed++ });
    const closing = client.destroy();
    const rejected = closing.catch((error: unknown) => error);
    transport.destruction.reject(new Error("cleanup failed"));
    expect(String(await rejected)).toContain("cleanup failed");
    expect(client.destroy()).toBe(closing);
    expect(states).toEqual(["idle"]);
    expect(errors.map((error) => error.code)).toEqual(["cleanup-failed"]);
    expect(completed).toBe(7);
    await expect(client.send("late")).rejects.toThrow("destroyed");
    transport.reads[0]?.reject(new Error("late read failure"));
    await settle();
    expect(transport.readCount).toBe(1);
  });

  test("malformed initialization releases the native owner before rejecting", async () => {
    const transport = new FakeTransport();
    transport.initial = { ...snapshot(0, "idle"), state: "unexpected" as NativeMediaState };
    await expect(NativeMediaClient.create({ transport })).rejects.toThrow("Invalid native");
    expect(transport.destroys).toBe(1);
    expect(transport.readCount).toBe(0);
  });

  test("initialization failure reports both the open and cleanup failures", async () => {
    const transport = new FakeTransport();
    transport.opening = deferred<NativeMediaSnapshot>();
    transport.destruction = deferred<NativeMediaSnapshot>();
    const creating = NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
    const errors: NativeMediaError[] = [];
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
    const client = await NativeMediaClient.create({ transport });
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
      const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
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
    const client = await NativeMediaClient.create({ transport });
    const states: NativeMediaState[] = [];
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
