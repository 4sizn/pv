export type CaptureKind = "camera" | "screen";

export interface CapturePort {
  readonly screenAvailable: boolean;
  capture(kind: CaptureKind, signal: AbortSignal): Promise<MediaStream>;
}

/** One browser acquisition operation, without retry or session policy. */
export class BrowserCapture implements CapturePort {
  get screenAvailable(): boolean {
    return typeof navigator.mediaDevices?.getDisplayMedia === "function";
  }

  async capture(kind: CaptureKind, signal: AbortSignal): Promise<MediaStream> {
    signal.throwIfAborted();
    if (!navigator.mediaDevices) {
      throw new Error(
        "이 환경에서는 미디어 캡처를 사용할 수 없습니다. localhost 또는 HTTPS 브라우저로 열어 주세요.",
      );
    }
    const stream =
      kind === "camera"
        ? await navigator.mediaDevices.getUserMedia({ video: true, audio: true })
        : await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
    if (signal.aborted) {
      for (const track of stream.getTracks()) track.stop();
      signal.throwIfAborted();
    }
    return stream;
  }
}
