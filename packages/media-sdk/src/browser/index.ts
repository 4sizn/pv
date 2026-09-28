import { MediaClient } from "../index.js";
import { BrowserPeerFactory } from "./peer.js";
import { BrowserSignalingAdapter } from "./signaling.js";

export interface BrowserMediaClientOptions {
  readonly signalingUrl: string;
  readonly iceServers?: readonly RTCIceServer[];
}

/** Composition root: the only place where the public client meets concrete adapters. */
export function createBrowserMediaClient(options: BrowserMediaClientOptions): MediaClient {
  return new MediaClient({
    signaling: new BrowserSignalingAdapter(options.signalingUrl),
    peers: new BrowserPeerFactory(options.iceServers),
  });
}

export { BrowserPeerFactory } from "./peer.js";
export { BrowserSignalingAdapter } from "./signaling.js";
export type { BrowserCaptureOptions } from "./tracks.js";
export {
  BrowserCaptureAdapter,
  captureCamera,
  captureMicrophone,
  captureScreen,
  fromBrowserTrack,
  toBrowserTrack,
} from "./tracks.js";
