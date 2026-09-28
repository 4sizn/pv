export type SourceKind = "camera" | "screen" | "microphone";
export type MediaState = "idle" | "joining" | "joined" | "leaving" | "destroyed";

/** An engine-owned handle. Concrete playback access belongs to the engine adapter. */
export interface MediaTrackPort {
  readonly id: string;
  readonly kind: "audio" | "video";
  stop(): void;
}

export interface JoinOptions {
  readonly roomId: string;
  readonly roomToken: string;
  readonly deviceToken: string;
}

export interface Publication {
  readonly id: string;
  readonly kind: SourceKind;
  readonly track: MediaTrackPort;
}

export interface RemoteTrack extends Publication {
  readonly peerId: string;
}

export interface MediaMessage {
  readonly peerId: string;
  readonly data: string;
}

export interface Description {
  readonly type: "offer" | "answer" | "rollback";
  readonly sdp?: string;
}

export interface IceCandidate {
  readonly candidate: string;
  readonly sdpMid?: string | null;
  readonly sdpMLineIndex?: number | null;
  readonly usernameFragment?: string | null;
}

export type SignalPayload =
  | { readonly type: "description"; readonly description: Description }
  | { readonly type: "ice"; readonly candidate: IceCandidate }
  | {
      readonly type: "track";
      readonly id: string;
      readonly kind: SourceKind;
      readonly mid: string;
    }
  | { readonly type: "track-removed"; readonly id: string };
