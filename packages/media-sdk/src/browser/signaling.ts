import { Subject } from "rxjs";
import type { JoinOptions, SignalingEvent, SignalingPort, SignalPayload } from "../index.js";

type Membership = { peerId: string; peers: readonly string[] };

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function parsePayload(value: unknown): SignalPayload {
  if (!record(value)) throw new Error("Invalid signal payload");
  if (value.type === "description" && record(value.description)) {
    const description = value.description;
    if (
      (description.type === "offer" || description.type === "answer") &&
      typeof description.sdp === "string"
    )
      return { type: "description", description: { type: description.type, sdp: description.sdp } };
  }
  if (value.type === "ice" && record(value.candidate)) {
    const candidate = value.candidate;
    if (
      typeof candidate.candidate === "string" &&
      (candidate.sdpMid === undefined ||
        candidate.sdpMid === null ||
        typeof candidate.sdpMid === "string") &&
      (candidate.sdpMLineIndex === undefined ||
        candidate.sdpMLineIndex === null ||
        typeof candidate.sdpMLineIndex === "number") &&
      (candidate.usernameFragment === undefined ||
        candidate.usernameFragment === null ||
        typeof candidate.usernameFragment === "string")
    ) {
      return {
        type: "ice",
        candidate: {
          candidate: candidate.candidate,
          sdpMid: candidate.sdpMid as string | null | undefined,
          sdpMLineIndex: candidate.sdpMLineIndex as number | null | undefined,
          usernameFragment: candidate.usernameFragment as string | null | undefined,
        },
      };
    }
  }
  if (
    value.type === "track" &&
    typeof value.id === "string" &&
    typeof value.mid === "string" &&
    (value.kind === "camera" || value.kind === "screen" || value.kind === "microphone")
  )
    return { type: "track", id: value.id, kind: value.kind, mid: value.mid };
  if (value.type === "track-removed" && typeof value.id === "string")
    return { type: "track-removed", id: value.id };
  throw new Error("Unsupported signal payload");
}

/** Owns a single socket/handshake and removes listeners on close. Never retries. */
export class BrowserSignalingAdapter implements SignalingPort {
  private readonly eventsSubject = new Subject<SignalingEvent>();
  readonly events$ = this.eventsSubject.asObservable();
  private socket?: WebSocket;
  private rejectPending?: (reason: Error) => void;
  private handshakeTimer?: ReturnType<typeof setTimeout>;

  constructor(private readonly url: string) {}

  connect(options: JoinOptions): Promise<Membership> {
    this.close();
    return new Promise<Membership>((resolve, reject) => {
      const socket = new WebSocket(this.url);
      this.socket = socket;
      this.rejectPending = reject;
      this.handshakeTimer = setTimeout(
        () => this.failHandshake(new Error("Signaling handshake timed out")),
        10_000,
      );
      socket.onopen = () => {
        if (socket === this.socket) socket.send(JSON.stringify({ type: "join", ...options }));
      };
      socket.onmessage = ({ data }: MessageEvent<unknown>) => {
        if (socket !== this.socket) return;
        try {
          if (typeof data !== "string" || data.length > 131_072)
            throw new Error("Invalid signaling message");
          const value: unknown = JSON.parse(data);
          if (!record(value)) throw new Error("Invalid signaling envelope");
          switch (value.type) {
            case "joined":
              if (
                !this.rejectPending ||
                typeof value.peerId !== "string" ||
                !Array.isArray(value.peers) ||
                value.peers.length > 3 ||
                !value.peers.every((id) => typeof id === "string")
              )
                throw new Error("Invalid membership response");
              clearTimeout(this.handshakeTimer);
              this.handshakeTimer = undefined;
              this.rejectPending = undefined;
              resolve({ peerId: value.peerId, peers: value.peers as string[] });
              break;
            case "peer-joined":
            case "peer-left":
              if (typeof value.peerId !== "string") throw new Error("Missing peer ID");
              this.eventsSubject.next({ type: value.type, peerId: value.peerId });
              break;
            case "signal":
              if (typeof value.from !== "string") throw new Error("Missing signal sender");
              this.eventsSubject.next({
                type: "signal",
                from: value.from,
                payload: parsePayload(value.payload),
              });
              break;
            case "error": {
              const error = new Error(
                typeof value.message === "string"
                  ? value.message
                  : "Signaling rejected the request",
              );
              if (this.rejectPending) this.failHandshake(error);
              else this.eventsSubject.next({ type: "error", error });
              break;
            }
            case "pong":
              break;
            default:
              throw new Error("Unsupported signaling message");
          }
        } catch (error) {
          const failure = error instanceof Error ? error : new Error(String(error));
          if (this.rejectPending) this.failHandshake(failure);
          else this.eventsSubject.next({ type: "error", error: failure });
        }
      };
      socket.onerror = () => {
        if (socket !== this.socket) return;
        if (this.rejectPending) this.failHandshake(new Error("Signaling connection failed"));
        else
          this.eventsSubject.next({
            type: "error",
            error: new Error("Signaling connection failed"),
          });
      };
      socket.onclose = () => {
        if (socket !== this.socket) return;
        this.close();
        this.eventsSubject.next({ type: "closed" });
      };
    });
  }

  send(to: string, payload: SignalPayload): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN)
      throw new Error("Signaling is not open");
    if (this.socket.bufferedAmount > 1_048_576) throw new Error("Signaling is backpressured");
    this.socket.send(JSON.stringify({ type: "signal", to, payload }));
  }

  ping(): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN)
      throw new Error("Signaling is not open");
    this.socket.send(JSON.stringify({ type: "ping" }));
  }

  close(): void {
    const socket = this.socket;
    this.socket = undefined;
    clearTimeout(this.handshakeTimer);
    this.handshakeTimer = undefined;
    const reject = this.rejectPending;
    this.rejectPending = undefined;
    reject?.(new Error("Signaling connection cancelled"));
    if (socket) {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      socket.close();
    }
  }

  private failHandshake(error: Error): void {
    const reject = this.rejectPending;
    this.rejectPending = undefined;
    reject?.(error);
    this.close();
  }
}
