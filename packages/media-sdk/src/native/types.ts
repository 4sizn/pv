import type { Observable } from "rxjs";

export type NativeDataState = "idle" | "joining" | "joined" | "leaving" | "destroyed";

export interface NativeDataSnapshot {
  readonly revision: number;
  readonly state: NativeDataState;
  readonly peerId: string | null;
  readonly peers: readonly string[];
  readonly readyPeers: readonly string[];
}

export interface NativeDataError {
  readonly code: string;
  readonly message: string;
}

export interface NativeDataMessage {
  readonly peerId: string;
  readonly data: string;
}

export type NativeDataEvent =
  | ({ readonly type: "message" } & NativeDataMessage)
  | { readonly type: "error"; readonly error: NativeDataError };

export interface NativeDataBatch {
  readonly snapshot: NativeDataSnapshot;
  readonly events: readonly NativeDataEvent[];
}

export interface NativeDataJoinOptions {
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
export interface NativeDataTransportPort {
  open(): Promise<NativeDataSnapshot>;
  readBatch(): Promise<NativeDataBatch>;
  join(options: NativeDataJoinOptions): Promise<NativeDataSnapshot>;
  send(data: string): Promise<NativeDataSendResult>;
  leave(): Promise<NativeDataSnapshot>;
  destroy(): Promise<NativeDataSnapshot>;
}

export interface NativeDataClientStreams {
  readonly state$: Observable<NativeDataState>;
  readonly peers$: Observable<readonly string[]>;
  readonly readyPeers$: Observable<readonly string[]>;
  readonly messages$: Observable<NativeDataMessage>;
  readonly errors$: Observable<NativeDataError>;
}
