import { Subject } from "rxjs";
import type {
  Description,
  IceCandidate,
  MediaTrackPort,
  PeerEvent,
  PeerFactoryPort,
  PeerPort,
} from "../index.js";
import { fromBrowserTrack, toBrowserTrack } from "./tracks.js";

/** One connection and its listeners. SDP ordering and offer policy belong to the controller. */
class BrowserPeer implements PeerPort {
  private readonly eventsSubject = new Subject<PeerEvent>();
  readonly events$ = this.eventsSubject.asObservable();
  private readonly senders = new Map<string, RTCRtpSender>();
  private readonly channels = new Set<RTCDataChannel>();
  private readonly trackListeners = new Map<MediaStreamTrack, () => void>();
  private dataChannel?: RTCDataChannel;
  private closed = false;

  constructor(private readonly connection: RTCPeerConnection) {
    connection.onnegotiationneeded = () => this.eventsSubject.next({ type: "negotiation-needed" });
    connection.onicecandidate = ({ candidate }) => {
      if (candidate)
        this.eventsSubject.next({
          type: "ice",
          candidate: {
            candidate: candidate.candidate,
            sdpMid: candidate.sdpMid,
            sdpMLineIndex: candidate.sdpMLineIndex,
            usernameFragment: candidate.usernameFragment,
          },
        });
    };
    connection.ontrack = ({ track, transceiver }) => {
      const mid = transceiver.mid;
      if (mid === null) return;
      const previous = this.trackListeners.get(track);
      if (previous) track.removeEventListener("ended", previous);
      const ended = () => this.eventsSubject.next({ type: "track-ended", mid });
      this.trackListeners.set(track, ended);
      track.addEventListener("ended", ended);
      this.eventsSubject.next({ type: "track", mid, track: fromBrowserTrack(track) });
    };
    connection.ondatachannel = ({ channel }) => this.bindChannel(channel);
    connection.onconnectionstatechange = () => {
      if (connection.connectionState === "failed")
        this.eventsSubject.next({ type: "failed", error: new Error("Peer connection failed") });
    };
  }

  get signalingState(): PeerPort["signalingState"] {
    return this.connection.signalingState as PeerPort["signalingState"];
  }
  get hasRemoteDescription(): boolean {
    return this.connection.remoteDescription !== null;
  }
  createDataChannel(): void {
    this.bindChannel(this.connection.createDataChannel("messages", { ordered: true }));
  }
  async createOffer(): Promise<Description> {
    const offer = await this.connection.createOffer();
    return { type: "offer", sdp: offer.sdp };
  }
  async createAnswer(): Promise<Description> {
    const answer = await this.connection.createAnswer();
    return { type: "answer", sdp: answer.sdp };
  }
  async setLocalDescription(description: Description): Promise<void> {
    await this.connection.setLocalDescription(description);
  }
  async setRemoteDescription(description: Description): Promise<void> {
    await this.connection.setRemoteDescription(description);
  }
  async addIceCandidate(candidate: IceCandidate): Promise<void> {
    await this.connection.addIceCandidate(candidate);
  }
  addTrack(id: string, track: MediaTrackPort): void {
    const native = toBrowserTrack(track);
    this.senders.set(id, this.connection.addTrack(native, new MediaStream([native])));
  }
  removeTrack(id: string): void {
    const sender = this.senders.get(id);
    if (!sender) return;
    this.connection.removeTrack(sender);
    this.senders.delete(id);
  }
  trackBindings(): readonly { id: string; mid: string }[] {
    const bindings: { id: string; mid: string }[] = [];
    for (const [id, sender] of this.senders) {
      const transceiver = this.connection
        .getTransceivers()
        .find((value) => value.sender === sender);
      if (transceiver?.mid !== null && transceiver?.mid !== undefined)
        bindings.push({ id, mid: transceiver.mid });
    }
    return bindings;
  }
  send(data: string): void {
    if (this.dataChannel?.readyState !== "open") throw new Error("Peer data channel is not open");
    if (this.dataChannel.bufferedAmount > 1_048_576)
      throw new Error("Peer data channel is backpressured");
    this.dataChannel.send(data);
  }
  close(): void {
    if (this.closed) return;
    this.closed = true;
    const connection = this.connection;
    connection.onnegotiationneeded = null;
    connection.onicecandidate = null;
    connection.ontrack = null;
    connection.ondatachannel = null;
    connection.onconnectionstatechange = null;
    for (const [track, ended] of this.trackListeners) track.removeEventListener("ended", ended);
    this.trackListeners.clear();
    for (const channel of this.channels) {
      channel.onmessage = null;
      channel.onerror = null;
      channel.close();
    }
    this.channels.clear();
    this.dataChannel = undefined;
    this.senders.clear();
    connection.close();
    this.eventsSubject.complete();
  }

  private bindChannel(channel: RTCDataChannel): void {
    if (this.closed) {
      channel.close();
      return;
    }
    this.channels.add(channel);
    this.dataChannel = channel;
    channel.onmessage = ({ data }: MessageEvent<unknown>) => {
      if (typeof data === "string" && data.length <= 16_384)
        this.eventsSubject.next({ type: "message", data });
    };
    channel.onerror = () =>
      this.eventsSubject.next({ type: "failed", error: new Error("Peer data channel failed") });
  }
}

export class BrowserPeerFactory implements PeerFactoryPort {
  constructor(private readonly iceServers: readonly RTCIceServer[] = []) {}
  create(_peerId: string): PeerPort {
    return new BrowserPeer(new RTCPeerConnection({ iceServers: [...this.iceServers] }));
  }
}
