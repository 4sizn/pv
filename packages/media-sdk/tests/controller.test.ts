import { describe, expect, test } from "bun:test";
import { Subject } from "rxjs";
import {
  type Description,
  type IceCandidate,
  MediaClient,
  type MediaState,
  type MediaTrackPort,
  type PeerEvent,
  type PeerFactoryPort,
  type PeerPort,
  type RemoteTrack,
  type SignalingEvent,
  type SignalingPort,
  type SignalPayload,
} from "../src/index.js";

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
const credentials = { roomId: "room", roomToken: "invitation", deviceToken: "device" };

class FakeTrack implements MediaTrackPort {
  stops = 0;
  constructor(
    readonly id: string,
    readonly kind: "audio" | "video" = "video",
  ) {}
  stop(): void {
    this.stops++;
  }
}
class FakeSignaling implements SignalingPort {
  readonly events = new Subject<SignalingEvent>();
  readonly events$ = this.events.asObservable();
  readonly sent: { to: string; payload: SignalPayload }[] = [];
  closed = 0;
  membership = deferred<{ peerId: string; peers: readonly string[] }>();
  connect() {
    return this.membership.promise;
  }
  send(to: string, payload: SignalPayload) {
    this.sent.push({ to, payload });
  }
  ping() {}
  close() {
    this.closed++;
  }
}
class FakePeer implements PeerPort {
  readonly events = new Subject<PeerEvent>();
  readonly events$ = this.events.asObservable();
  signalingState: PeerPort["signalingState"] = "stable";
  hasRemoteDescription = false;
  closed = 0;
  channels = 0;
  failAdd = false;
  offerGate?: ReturnType<typeof deferred<Description>>;
  readonly mutations: string[] = [];
  readonly attached = new Map<string, MediaTrackPort>();
  createDataChannel() {
    this.channels++;
  }
  async createOffer(): Promise<Description> {
    this.mutations.push("offer");
    return this.offerGate ? this.offerGate.promise : { type: "offer", sdp: "local" };
  }
  async createAnswer(): Promise<Description> {
    this.mutations.push("answer");
    return { type: "answer", sdp: "local" };
  }
  async setLocalDescription(description: Description) {
    this.mutations.push(`local:${description.type}`);
    this.signalingState = description.type === "offer" ? "have-local-offer" : "stable";
  }
  async setRemoteDescription(description: Description) {
    this.mutations.push(`remote:${description.type}`);
    this.hasRemoteDescription = true;
    this.signalingState = description.type === "offer" ? "have-remote-offer" : "stable";
  }
  async addIceCandidate(candidate: IceCandidate) {
    if (!this.hasRemoteDescription) throw new Error("ICE before SDP");
    this.mutations.push(`ice:${candidate.candidate}`);
  }
  addTrack(id: string, track: MediaTrackPort) {
    if (this.failAdd) throw new Error("track rejected");
    this.attached.set(id, track);
  }
  removeTrack(id: string) {
    this.attached.delete(id);
  }
  trackBindings() {
    return [...this.attached.keys()].map((id, index) => ({ id, mid: String(index) }));
  }
  send(_data: string) {}
  close() {
    this.closed++;
    this.signalingState = "closed";
  }
}
class FakeFactory implements PeerFactoryPort {
  readonly peers = new Map<string, FakePeer>();
  create(id: string) {
    const peer = new FakePeer();
    this.peers.set(id, peer);
    return peer;
  }
}
async function setup(selfId = "b", peerIds = ["a"]) {
  const signaling = new FakeSignaling();
  const peers = new FakeFactory();
  const client = new MediaClient({ signaling, peers });
  const joining = client.join(credentials);
  signaling.membership.resolve({ peerId: selfId, peers: peerIds });
  await joining;
  return { signaling, peers, client, peer: peers.peers.get(peerIds[0] ?? "") as FakePeer };
}

describe("media controller resource ownership", () => {
  test("leave observers share teardown without recursive cleanup", async () => {
    const { client, signaling, peer } = await setup();
    const local = new FakeTrack("owned");
    await client.publish({ id: "camera", kind: "camera", track: local });
    let notifications = 0;
    let nested: Promise<void> | undefined;
    client.state$.subscribe((state) => {
      if (state === "leaving") {
        notifications++;
        // A bound keeps the regression failure finite on an unguarded controller.
        if (notifications < 5) nested = client.leave();
      }
    });
    const leaving = client.leave();
    await leaving;
    expect(notifications).toBe(1);
    expect(nested).toBe(leaving);
    expect(signaling.closed).toBe(1);
    expect(peer.closed).toBe(1);
    expect(local.stops).toBe(1);
    await client.destroy();
  });

  test("concurrent and reentrant destroy share completion after streams complete", async () => {
    const { client, signaling } = await setup();
    const completed: string[] = [];
    let nested: Promise<void> | undefined;
    client.state$.subscribe({
      next: (state) => {
        if (state === "leaving") nested = client.destroy();
      },
      complete: () => completed.push("state"),
    });
    client.peers$.subscribe({ complete: () => completed.push("peers") });
    client.tracks$.subscribe({ complete: () => completed.push("tracks") });
    client.errors$.subscribe({ complete: () => completed.push("errors") });
    client.messages$.subscribe({ complete: () => completed.push("messages") });
    const first = client.destroy();
    const second = client.destroy();
    expect(second).toBe(first);
    expect(nested).toBe(first);
    await second;
    expect(completed).toEqual(["state", "peers", "tracks", "errors", "messages"]);
    expect(signaling.closed).toBe(1);
    expect(client.destroy()).toBe(first);
  });

  test("leave from a state observer cannot resurrect a cancelled join", async () => {
    const signaling = new FakeSignaling();
    const peers = new FakeFactory();
    const client = new MediaClient({ signaling, peers });
    client.state$.subscribe((state) => {
      if (state === "joined") void client.leave();
    });
    const joining = client.join(credentials);
    signaling.membership.resolve({ peerId: "a", peers: ["b", "c"] });
    await expect(joining).rejects.toThrow("cancelled");
    expect(peers.peers.size).toBe(0);
    await client.destroy();
  });
  test("leave cancels unresolved join promptly and ignores late completion", async () => {
    const signaling = new FakeSignaling();
    const peers = new FakeFactory();
    const client = new MediaClient({ signaling, peers });
    const states: MediaState[] = [];
    client.state$.subscribe((state) => states.push(state));
    const joining = client.join(credentials);
    const rejection = joining.catch((error: Error) => error);
    await client.leave();
    expect(((await rejection) as Error).message).toContain("cancelled");
    signaling.membership.resolve({ peerId: "self", peers: ["remote"] });
    await settle();
    expect(peers.peers.size).toBe(0);
    expect(states.at(-1)).toBe("idle");
    expect(signaling.closed).toBe(1);
    await client.destroy();
  });

  test("late offer cannot mutate a closed peer or send on a later session", async () => {
    const { client, signaling, peer } = await setup();
    peer.offerGate = deferred<Description>();
    peer.events.next({ type: "negotiation-needed" });
    await settle();
    await client.leave();
    peer.offerGate.resolve({ type: "offer", sdp: "stale" });
    await settle();
    expect(peer.mutations).toEqual(["offer"]);
    expect(signaling.sent).toEqual([]);
    await client.destroy();
  });

  test("peer departure stops only its remote tracks; leave stops owned publications once", async () => {
    const { client, signaling, peer } = await setup();
    const local = new FakeTrack("local");
    const remote = new FakeTrack("remote");
    let tracks: readonly RemoteTrack[] = [];
    client.tracks$.subscribe((value) => {
      tracks = value;
    });
    await client.publish({ id: "camera", kind: "camera", track: local });
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "track", id: "screen", kind: "screen", mid: "0" },
    });
    peer.events.next({ type: "track", mid: "0", track: remote });
    await settle();
    expect(tracks).toHaveLength(1);
    signaling.events.next({ type: "peer-left", peerId: "a" });
    expect(tracks).toEqual([]);
    expect(remote.stops).toBe(1);
    expect(local.stops).toBe(0);
    expect(peer.closed).toBe(1);
    await client.leave();
    await client.leave();
    await client.destroy();
    await client.destroy();
    expect(local.stops).toBe(1);
    expect(remote.stops).toBe(1);
    await expect(client.join(credentials)).rejects.toThrow("destroyed");
  });

  test("failed publication rolls back peer attachments and retains caller ownership", async () => {
    const { client, peers, signaling } = await setup("a", ["b", "c"]);
    const failing = peers.peers.get("c") as FakePeer;
    failing.failAdd = true;
    const track = new FakeTrack("local");
    await expect(client.publish({ id: "camera", kind: "camera", track })).rejects.toThrow(
      "track rejected",
    );
    expect(peers.peers.get("b")?.attached.size).toBe(0);
    expect(signaling.sent.filter(({ payload }) => payload.type === "track-removed")).toHaveLength(
      2,
    );
    await client.destroy();
    expect(track.stops).toBe(0);
  });

  test("failed join resets lifecycle and supports another join", async () => {
    const signaling = new FakeSignaling();
    const client = new MediaClient({ signaling, peers: new FakeFactory() });
    const rejected = client.join(credentials);
    signaling.membership.reject(new Error("denied"));
    await expect(rejected).rejects.toThrow("denied");
    signaling.membership = deferred();
    const retry = client.join(credentials);
    signaling.membership.resolve({ peerId: "a", peers: [] });
    await retry;
    await client.destroy();
  });
});

describe("serialized negotiation", () => {
  test("publication metadata uses its negotiated binding after local description", async () => {
    const { client, signaling, peer } = await setup();
    await client.publish({
      id: "camera",
      kind: "camera",
      track: new FakeTrack("sender-native-id"),
    });
    expect(signaling.sent).toEqual([]);
    peer.events.next({ type: "negotiation-needed" });
    await settle();
    expect(signaling.sent.map(({ payload }) => payload.type)).toEqual(["track", "description"]);
    expect(signaling.sent[0]?.payload).toEqual({
      type: "track",
      id: "camera",
      kind: "camera",
      mid: "0",
    });
    expect(peer.mutations).toEqual(["offer", "local:offer"]);
    await client.destroy();
  });

  test("republished source can reuse a live receiver with a different native track ID", async () => {
    const { client, signaling, peer } = await setup();
    const receiver = new FakeTrack("receiver-native-id");
    let tracks: readonly RemoteTrack[] = [];
    client.tracks$.subscribe((value) => {
      tracks = value;
    });
    peer.events.next({ type: "track", mid: "2", track: receiver });
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "track", id: "camera-first", kind: "camera", mid: "2" },
    });
    await settle();
    expect(tracks[0]?.track).toBe(receiver);
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "track-removed", id: "camera-first" },
    });
    await settle();
    expect(tracks).toEqual([]);
    expect(receiver.stops).toBe(0);
    // No second engine track event is required when the transceiver retains its receiver.
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "track", id: "camera-second", kind: "camera", mid: "2" },
    });
    await settle();
    expect(tracks[0]).toEqual({
      peerId: "a",
      id: "camera-second",
      kind: "camera",
      track: receiver,
    });
    await client.destroy();
    expect(receiver.stops).toBe(1);
  });

  test("buffers early ICE and applies it after the remote offer", async () => {
    const { client, signaling, peer } = await setup();
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "ice", candidate: { candidate: "early" } },
    });
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "description", description: { type: "offer", sdp: "remote" } },
    });
    await settle();
    expect(peer.mutations).toEqual(["remote:offer", "ice:early", "answer", "local:answer"]);
    await client.destroy();
  });

  test("polite peer rolls back an offer collision in its queue", async () => {
    const { client, signaling, peer } = await setup("b", ["a"]);
    peer.events.next({ type: "negotiation-needed" });
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "description", description: { type: "offer", sdp: "remote" } },
    });
    await settle();
    expect(peer.mutations).toEqual([
      "offer",
      "local:offer",
      "local:rollback",
      "remote:offer",
      "answer",
      "local:answer",
    ]);
    await client.destroy();
  });

  test("impolite peer ignores collided offer and its ICE, then accepts answer", async () => {
    const { client, signaling, peer } = await setup("a", ["b"]);
    peer.events.next({ type: "negotiation-needed" });
    signaling.events.next({
      type: "signal",
      from: "b",
      payload: { type: "description", description: { type: "offer", sdp: "collision" } },
    });
    signaling.events.next({
      type: "signal",
      from: "b",
      payload: { type: "ice", candidate: { candidate: "ignored" } },
    });
    signaling.events.next({
      type: "signal",
      from: "b",
      payload: { type: "description", description: { type: "answer", sdp: "answer" } },
    });
    await settle();
    expect(peer.mutations).toEqual(["offer", "local:offer", "remote:answer"]);
    expect(peer.channels).toBe(1);
    await client.destroy();
  });

  test("track changes during an offer trigger negotiation after its answer", async () => {
    const { client, signaling, peer } = await setup();
    peer.events.next({ type: "negotiation-needed" });
    await settle();
    peer.events.next({ type: "negotiation-needed" });
    signaling.events.next({
      type: "signal",
      from: "a",
      payload: { type: "description", description: { type: "answer", sdp: "answer" } },
    });
    await settle();
    expect(peer.mutations).toEqual([
      "offer",
      "local:offer",
      "remote:answer",
      "offer",
      "local:offer",
    ]);
    await client.destroy();
  });
});
