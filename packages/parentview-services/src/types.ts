import type { JoinOptions } from "@parentview/media-sdk";
import type { Observable } from "rxjs";

export type ParentViewRole = "parent" | "child";
export type Participant = Readonly<{ peerId: string; role: ParentViewRole }>;

/** Supplied by authenticated pairing/application composition, never a peer's self-declared message. */
export type SessionContext = Readonly<{
  local: Participant;
  participants: readonly Participant[];
  connection: JoinOptions;
}>;

export interface MediaSessionPort {
  join(options: JoinOptions): Promise<void>;
  leave(): Promise<void>;
}

export type RemoteInput =
  | Readonly<{ type: "pointer"; x: number; y: number; action: "move" | "down" | "up" }>
  | Readonly<{ type: "key"; key: string; action: "down" | "up" }>;

/** OS input integration must validate support and OS permission on every execution. */
export interface RemoteInputPort {
  isAvailable(): boolean;
  execute(input: RemoteInput): Promise<void>;
  releaseAll(): Promise<void>;
}

export type SupportSnapshot = Readonly<{
  approval: "idle" | "pending" | "approved" | "rejected";
  connection: "idle" | "joining" | "active" | "ending" | "ended";
  screenConsent: "not-granted" | "granted" | "revoked";
  controllerPeerId: string | null;
}>;

export type NetworkSnapshot = Readonly<{ availability: "online" | "offline" | "unknown" }>;
export interface NetworkEnvironmentPort {
  current(): NetworkSnapshot;
  changes$: Observable<NetworkSnapshot>;
}

export type Device = Readonly<{
  id: string;
  kind: "camera" | "microphone" | "speaker";
  label: string;
}>;
export interface DeviceCatalogPort {
  changes$: Observable<void>;
  enumerate(): Promise<readonly Device[]>;
}
