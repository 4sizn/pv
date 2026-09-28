import type { CapturePort, MediaTrackPort, SourceKind } from "../index.js";

class BrowserMediaTrack implements MediaTrackPort {
  constructor(readonly native: MediaStreamTrack) {}
  get id(): string {
    return this.native.id;
  }
  get kind(): "audio" | "video" {
    return this.native.kind as "audio" | "video";
  }
  stop(): void {
    this.native.stop();
  }
}

export function fromBrowserTrack(track: MediaStreamTrack): MediaTrackPort {
  return new BrowserMediaTrack(track);
}

export function toBrowserTrack(track: MediaTrackPort): MediaStreamTrack {
  if (!(track instanceof BrowserMediaTrack))
    throw new Error("Track does not belong to the browser adapter");
  return track.native;
}

export interface BrowserCaptureOptions {
  readonly signal?: AbortSignal;
}

/** Owns one capture request. Returned handles transfer to the caller; no retry policy. */
export class BrowserCaptureAdapter implements CapturePort {
  constructor(private readonly options: BrowserCaptureOptions = {}) {}

  async capture(kind: SourceKind): Promise<MediaTrackPort[]> {
    this.options.signal?.throwIfAborted();
    const stream =
      kind === "screen"
        ? await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false })
        : await navigator.mediaDevices.getUserMedia({
            video: kind === "camera",
            audio: kind === "microphone",
          });
    if (this.options.signal?.aborted) {
      for (const track of stream.getTracks()) track.stop();
      this.options.signal.throwIfAborted();
    }
    return stream.getTracks().map(fromBrowserTrack);
  }
}

export function captureCamera(options?: BrowserCaptureOptions): Promise<MediaTrackPort[]> {
  return new BrowserCaptureAdapter(options).capture("camera");
}
export function captureScreen(options?: BrowserCaptureOptions): Promise<MediaTrackPort[]> {
  return new BrowserCaptureAdapter(options).capture("screen");
}
export function captureMicrophone(options?: BrowserCaptureOptions): Promise<MediaTrackPort[]> {
  return new BrowserCaptureAdapter(options).capture("microphone");
}
