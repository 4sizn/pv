import type { Observable } from "rxjs";
import type {
  Description,
  IceCandidate,
  JoinOptions,
  MediaTrackPort,
  SignalPayload,
  SourceKind,
} from "./types.js";

export type SignalingEvent =
  | { readonly type: "peer-joined"; readonly peerId: string }
  | { readonly type: "peer-left"; readonly peerId: string }
  | { readonly type: "signal"; readonly from: string; readonly payload: SignalPayload }
  | { readonly type: "error"; readonly error: Error }
  | { readonly type: "closed" };

export interface SignalingPort {
  readonly events$: Observable<SignalingEvent>;
  connect(options: JoinOptions): Promise<{ peerId: string; peers: readonly string[] }>;
  send(to: string, payload: SignalPayload): void;
  ping(): void;
  /** Synchronously invalidates pending connect and detaches resources. Reusable. */
  close(): void;
}

export type PeerEvent =
  | { readonly type: "negotiation-needed" }
  | { readonly type: "ice"; readonly candidate: IceCandidate }
  | { readonly type: "track"; readonly mid: string; readonly track: MediaTrackPort }
  | { readonly type: "track-ended"; readonly mid: string }
  | { readonly type: "message"; readonly data: string }
  | { readonly type: "failed"; readonly error: Error };

export interface PeerPort {
  readonly events$: Observable<PeerEvent>;
  readonly signalingState: "stable" | "have-local-offer" | "have-remote-offer" | "closed";
  readonly hasRemoteDescription: boolean;
  createDataChannel(): void;
  createOffer(): Promise<Description>;
  createAnswer(): Promise<Description>;
  setLocalDescription(description: Description): Promise<void>;
  setRemoteDescription(description: Description): Promise<void>;
  addIceCandidate(candidate: IceCandidate): Promise<void>;
  addTrack(id: string, track: MediaTrackPort): void;
  removeTrack(id: string): void;
  /** Negotiated engine binding, available after local description application. */
  trackBindings(): readonly { id: string; mid: string }[];
  send(data: string): void;
  close(): void;
}

export interface PeerFactoryPort {
  create(peerId: string): PeerPort;
}

/** Capture is optional and remains outside the controller's session state. */
export interface CapturePort {
  capture(kind: SourceKind): Promise<MediaTrackPort[]>;
}

export interface MediaClientDependencies {
  readonly signaling: SignalingPort;
  readonly peers: PeerFactoryPort;
}
