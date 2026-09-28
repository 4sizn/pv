import type { Observable } from "rxjs";

export type NativeMediaState = "idle" | "joining" | "joined" | "leaving" | "destroyed";

export interface NativeLocalSource {
  readonly id: string;
  readonly kind: "camera" | "screen" | "microphone";
}

/** Source metadata only. Frames and playback resources remain in the native engine. */
export interface NativeRemoteSource extends NativeLocalSource {
  readonly peerId: string;
  readonly mid: string;
}

export interface NativeMediaSnapshot {
  readonly revision: number;
  readonly state: NativeMediaState;
  readonly peerId: string | null;
  readonly peers: readonly string[];
  readonly readyPeers: readonly string[];
  readonly localSources: readonly NativeLocalSource[];
  readonly remoteSources: readonly NativeRemoteSource[];
}

export interface NativeMediaError {
  readonly code: string;
  readonly message: string;
}

export interface NativeDataMessage {
  readonly peerId: string;
  readonly data: string;
}

export type NativeMediaEvent =
  | ({ readonly type: "message" } & NativeDataMessage)
  | { readonly type: "error"; readonly error: NativeMediaError };

export interface NativeMediaBatch {
  readonly snapshot: NativeMediaSnapshot;
  readonly events: readonly NativeMediaEvent[];
}

export interface NativeMediaJoinOptions {
  readonly signalingUrl: string;
  readonly roomId: string;
  readonly roomToken: string;
  readonly deviceToken: string;
  readonly iceServers?: readonly {
    readonly urls: readonly string[];
    readonly username?: string;
    readonly credential?: string;
  }[];
}

/** Local engine acceptance per peer; this does not acknowledge remote application delivery. */
export interface NativeDataSendResult {
  readonly acceptedPeerIds: readonly string[];
  readonly failures: readonly { readonly peerId: string; readonly message: string }[];
}

/**
 * One native client, injected by the host. The native runtime owns all media state.
 * readBatch returns at most 32 events and only one read may be pending.
 * destroy cancels native work, releases resources and wakes pending reads, even on failure.
 */
export interface NativeMediaTransportPort {
  open(): Promise<NativeMediaSnapshot>;
  readBatch(): Promise<NativeMediaBatch>;
  join(options: NativeMediaJoinOptions): Promise<NativeMediaSnapshot>;
  send(data: string): Promise<NativeDataSendResult>;
  leave(): Promise<NativeMediaSnapshot>;
  destroy(): Promise<NativeMediaSnapshot>;
}

export interface NativeMediaClientStreams {
  readonly state$: Observable<NativeMediaState>;
  readonly peers$: Observable<readonly string[]>;
  readonly readyPeers$: Observable<readonly string[]>;
  readonly localSources$: Observable<readonly NativeLocalSource[]>;
  readonly remoteSources$: Observable<readonly NativeRemoteSource[]>;
  readonly messages$: Observable<NativeDataMessage>;
  readonly errors$: Observable<NativeMediaError>;
}
