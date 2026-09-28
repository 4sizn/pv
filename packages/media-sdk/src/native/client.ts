import { BehaviorSubject, distinctUntilChanged, map, Subject } from "rxjs";
import type {
  NativeDataMessage,
  NativeDataSendResult,
  NativeLocalSource,
  NativeMediaBatch,
  NativeMediaClientStreams,
  NativeMediaError,
  NativeMediaEvent,
  NativeMediaJoinOptions,
  NativeMediaSnapshot,
  NativeMediaState,
  NativeMediaTransportPort,
  NativeRemoteSource,
} from "./types.js";

const MAX_DATA_BYTES = 16_384;
const STATES: readonly NativeMediaState[] = ["idle", "joining", "joined", "leaving", "destroyed"];

/**
 * Projects native snapshots/events into Rx. Owns only the IPC read loop and its streams,
 * never session transitions or negotiation. destroy is terminal even if host cleanup fails.
 */
export class NativeMediaClient implements NativeMediaClientStreams {
  readonly state$;
  readonly peers$;
  readonly readyPeers$;
  readonly localSources$;
  readonly remoteSources$;
  readonly messages$;
  readonly errors$;
  #transport: NativeMediaTransportPort;
  #snapshots: BehaviorSubject<NativeMediaSnapshot>;
  #messages = new Subject<NativeDataMessage>();
  #errors = new Subject<NativeMediaError>();
  #disposed = false;
  #destruction?: Promise<void>;
  #pending = new Set<() => void>();

  private constructor(transport: NativeMediaTransportPort, snapshot: NativeMediaSnapshot) {
    this.#transport = transport;
    this.#snapshots = new BehaviorSubject(snapshot);
    this.state$ = this.#snapshots.asObservable().pipe(
      map((value) => value.state),
      distinctUntilChanged(),
    );
    this.peers$ = this.#snapshots.asObservable().pipe(
      map((value) => value.peers),
      distinctUntilChanged(samePeers),
    );
    this.readyPeers$ = this.#snapshots.asObservable().pipe(
      map((value) => value.readyPeers),
      distinctUntilChanged(samePeers),
    );
    this.localSources$ = this.#snapshots.asObservable().pipe(
      map((value) => value.localSources),
      distinctUntilChanged(sameLocalSources),
    );
    this.remoteSources$ = this.#snapshots.asObservable().pipe(
      map((value) => value.remoteSources),
      distinctUntilChanged(sameRemoteSources),
    );
    this.messages$ = this.#messages.asObservable();
    this.errors$ = this.#errors.asObservable();
  }

  static async create(options: {
    transport: NativeMediaTransportPort;
  }): Promise<NativeMediaClient> {
    const { transport } = options;
    let snapshot: NativeMediaSnapshot;
    try {
      snapshot = parseSnapshot(await transport.open());
      if (snapshot.state === "destroyed") throw new Error("Native client is already destroyed");
    } catch (error) {
      try {
        await transport.destroy();
      } catch (cleanupError) {
        throw new AggregateError([error, cleanupError], "Native client initialization failed");
      }
      throw error;
    }
    const client = new NativeMediaClient(transport, snapshot);
    void client.#read();
    return client;
  }

  async join(options: NativeMediaJoinOptions): Promise<void> {
    this.#assertAlive();
    const snapshot = await this.#wait(this.#transport.join(options));
    this.#assertAlive();
    this.#apply(parseSnapshot(snapshot));
  }

  async send(data: string): Promise<NativeDataSendResult> {
    this.#assertAlive();
    if (typeof data !== "string" || byteLength(data) > MAX_DATA_BYTES)
      throw new Error("Native data message exceeds 16384 UTF-8 bytes");
    const result = await this.#wait(this.#transport.send(data));
    this.#assertAlive();
    return parseSendResult(result);
  }

  async leave(): Promise<void> {
    this.#assertAlive();
    const snapshot = await this.#wait(this.#transport.leave());
    this.#assertAlive();
    this.#apply(parseSnapshot(snapshot));
  }

  destroy(): Promise<void> {
    if (this.#destruction) return this.#destruction;
    this.#disposed = true;
    for (const cancel of this.#pending) cancel();
    // Install before executing transport code or notifying observers, allowing reentrant teardown.
    this.#destruction = Promise.resolve()
      .then(async () => {
        const snapshot = parseSnapshot(await this.#transport.destroy());
        if (snapshot.state !== "destroyed") throw invalidPayload();
        if (!this.#apply(snapshot, true)) throw invalidPayload();
      })
      .catch((error: unknown) => {
        this.#errors.next(
          Object.freeze({ code: "cleanup-failed", message: "Native client cleanup failed" }),
        );
        throw error;
      })
      .finally(() => {
        this.#snapshots.complete();
        this.#messages.complete();
        this.#errors.complete();
      });
    return this.#destruction;
  }

  #wait<T>(operation: Promise<T>): Promise<T> {
    // Remove cancellation callbacks after every response. Racing each read against a shared
    // pending teardown promise would retain a reaction for every batch until destruction.
    return new Promise<T>((resolve, reject) => {
      const cancel = () => {
        if (this.#pending.delete(cancel)) reject(new Error("Native data client is destroyed"));
      };
      this.#pending.add(cancel);
      Promise.resolve(operation).then(
        (value) => {
          if (this.#pending.delete(cancel)) resolve(value);
        },
        (error: unknown) => {
          if (this.#pending.delete(cancel)) reject(error);
        },
      );
      if (this.#disposed) cancel();
    });
  }

  async #read(): Promise<void> {
    try {
      while (!this.#disposed) {
        const batch = parseBatch(await this.#wait(this.#transport.readBatch()));
        if (!this.#apply(batch.snapshot)) continue;
        if (batch.snapshot.state === "destroyed") {
          await this.destroy();
          return;
        }
        for (const event of batch.events) {
          if (this.#disposed) break;
          if (event.type === "message") {
            this.#messages.next(Object.freeze({ peerId: event.peerId, data: event.data }));
          } else this.#errors.next(event.error);
        }
      }
    } catch {
      if (this.#disposed) return;
      this.#errors.next(
        Object.freeze({ code: "transport-failed", message: "Native event transport failed" }),
      );
      // Loss of the only event feed must release its native owner, without a competing retry loop.
      await this.destroy().catch(() => {});
    }
  }

  #apply(snapshot: NativeMediaSnapshot, terminal = false): boolean {
    if (this.#disposed && !terminal) return false;
    const previous = this.#snapshots.value;
    if (snapshot.revision < previous.revision) return false;
    if (snapshot.revision === previous.revision) {
      if (
        snapshot.state !== previous.state ||
        snapshot.peerId !== previous.peerId ||
        !samePeers(snapshot.peers, previous.peers) ||
        !samePeers(snapshot.readyPeers, previous.readyPeers) ||
        !sameLocalSources(snapshot.localSources, previous.localSources) ||
        !sameRemoteSources(snapshot.remoteSources, previous.remoteSources)
      ) {
        throw invalidPayload();
      }
      return true;
    }
    this.#snapshots.next(snapshot);
    return true;
  }

  #assertAlive(): void {
    if (this.#disposed) throw new Error("Native data client is destroyed");
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonempty(value: unknown, max = 512): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}

function invalidPayload(): Error {
  return new Error("Invalid native transport payload");
}

function samePeers(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((id, index) => id === b[index]);
}

function sameLocalSources(
  a: readonly NativeLocalSource[],
  b: readonly NativeLocalSource[],
): boolean {
  return (
    a.length === b.length &&
    a.every((source, index) => source.id === b[index].id && source.kind === b[index].kind)
  );
}

function sameRemoteSources(
  a: readonly NativeRemoteSource[],
  b: readonly NativeRemoteSource[],
): boolean {
  return (
    sameLocalSources(a, b) &&
    a.every((source, index) => source.peerId === b[index].peerId && source.mid === b[index].mid)
  );
}

function sourceIdentifier(value: unknown): value is string {
  return nonempty(value, 128) && byteLength(value) <= 128 && !/\p{Cc}/u.test(value);
}

function parseSource(value: unknown): NativeLocalSource {
  if (
    !record(value) ||
    !sourceIdentifier(value.id) ||
    (value.kind !== "camera" && value.kind !== "screen" && value.kind !== "microphone")
  ) {
    throw invalidPayload();
  }
  return Object.freeze({ id: value.id, kind: value.kind });
}

function parseLocalSources(value: unknown): readonly NativeLocalSource[] {
  if (!Array.isArray(value) || value.length > 3) throw invalidPayload();
  const sources = Array.from(value, parseSource);
  if (
    new Set(sources.map((source) => source.id)).size !== sources.length ||
    new Set(sources.map((source) => source.kind)).size !== sources.length
  ) {
    throw invalidPayload();
  }
  return Object.freeze(sources);
}

function parseRemoteSources(
  value: unknown,
  peers: readonly string[],
): readonly NativeRemoteSource[] {
  if (!Array.isArray(value) || value.length > 9) throw invalidPayload();
  const sources = Array.from(value, (value: unknown) => {
    const source = parseSource(value);
    if (
      !record(value) ||
      typeof value.peerId !== "string" ||
      !peers.includes(value.peerId) ||
      !sourceIdentifier(value.mid)
    ) {
      throw invalidPayload();
    }
    return Object.freeze({ ...source, peerId: value.peerId, mid: value.mid });
  });
  for (const field of ["id", "kind", "mid"] as const) {
    const keys = sources.map((source) => JSON.stringify([source.peerId, source[field]]));
    if (new Set(keys).size !== sources.length) throw invalidPayload();
  }
  return Object.freeze(sources);
}

function parsePeers(value: unknown): readonly string[] {
  if (
    !Array.isArray(value) ||
    value.length > 3 ||
    !value.every((id) => nonempty(id)) ||
    new Set(value).size !== value.length
  ) {
    throw invalidPayload();
  }
  return Object.freeze([...value]);
}

function parseSnapshot(value: unknown): NativeMediaSnapshot {
  if (
    !record(value) ||
    typeof value.revision !== "number" ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 0 ||
    !STATES.includes(value.state as NativeMediaState) ||
    (value.peerId !== null && !nonempty(value.peerId))
  ) {
    throw invalidPayload();
  }
  const peers = parsePeers(value.peers);
  const readyPeers = parsePeers(value.readyPeers);
  const localSources = parseLocalSources(value.localSources);
  const remoteSources = parseRemoteSources(value.remoteSources, peers);
  if (
    (value.state === "joined" && value.peerId === null) ||
    readyPeers.some((id) => !peers.includes(id)) ||
    peers.includes(value.peerId as string) ||
    ((value.state === "idle" || value.state === "joining" || value.state === "destroyed") &&
      (localSources.length > 0 || remoteSources.length > 0)) ||
    ((value.state === "idle" || value.state === "destroyed") &&
      (value.peerId !== null || peers.length > 0))
  ) {
    throw invalidPayload();
  }
  return Object.freeze({
    revision: value.revision,
    state: value.state as NativeMediaState,
    peerId: value.peerId as string | null,
    peers,
    readyPeers,
    localSources,
    remoteSources,
  });
}

function parseEvent(value: unknown): NativeMediaEvent {
  if (!record(value)) throw invalidPayload();
  if (
    value.type === "message" &&
    nonempty(value.peerId) &&
    typeof value.data === "string" &&
    byteLength(value.data) <= MAX_DATA_BYTES
  ) {
    return Object.freeze({ type: "message", peerId: value.peerId, data: value.data });
  }
  if (
    value.type === "error" &&
    record(value.error) &&
    nonempty(value.error.code, 128) &&
    nonempty(value.error.message, 4096)
  ) {
    return Object.freeze({
      type: "error",
      error: Object.freeze({ code: value.error.code, message: value.error.message }),
    });
  }
  throw invalidPayload();
}

function parseBatch(value: unknown): NativeMediaBatch {
  if (!record(value) || !Array.isArray(value.events) || value.events.length > 32)
    throw invalidPayload();
  return { snapshot: parseSnapshot(value.snapshot), events: value.events.map(parseEvent) };
}

function parseSendResult(value: unknown): NativeDataSendResult {
  if (!record(value) || !Array.isArray(value.failures) || value.failures.length > 3)
    throw invalidPayload();
  const acceptedPeerIds = parsePeers(value.acceptedPeerIds);
  const failures = value.failures.map((failure: unknown) => {
    if (!record(failure) || !nonempty(failure.peerId) || !nonempty(failure.message, 4096))
      throw invalidPayload();
    return Object.freeze({ peerId: failure.peerId, message: failure.message });
  });
  const ids = [...acceptedPeerIds, ...failures.map((failure) => failure.peerId)];
  if (ids.length > 3 || new Set(ids).size !== ids.length) throw invalidPayload();
  return Object.freeze({ acceptedPeerIds, failures: Object.freeze(failures) });
}

function byteLength(value: string): number {
  let bytes = 0;
  for (const character of value) {
    const point = character.codePointAt(0) ?? 0;
    bytes += point <= 0x7f ? 1 : point <= 0x7ff ? 2 : point <= 0xffff ? 3 : 4;
    if (bytes > MAX_DATA_BYTES) break;
  }
  return bytes;
}
