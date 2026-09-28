import { BehaviorSubject, interval, Subject, type Subscription } from "rxjs";
import type { MediaClientDependencies, PeerPort, SignalingEvent } from "./ports.js";
import type {
  IceCandidate,
  JoinOptions,
  MediaMessage,
  MediaState,
  MediaTrackPort,
  Publication,
  RemoteTrack,
  SignalPayload,
  SourceKind,
} from "./types.js";

interface PeerSession {
  readonly id: string;
  readonly port: PeerPort;
  readonly generation: number;
  readonly polite: boolean;
  subscription?: Subscription;
  queue: Promise<void>;
  closed: boolean;
  ignoreOffer: boolean;
  negotiationRequested: boolean;
  readonly earlyIce: IceCandidate[];
  readonly metadata: Map<string, { kind: SourceKind; mid: string }>;
  readonly incoming: Map<string, MediaTrackPort>;
}

/** Owns session generations, publications and all negotiation policy. No engine globals. */
export class MediaController {
  private readonly stateSubject = new BehaviorSubject<MediaState>("idle");
  private readonly peersSubject = new BehaviorSubject<readonly string[]>([]);
  private readonly tracksSubject = new BehaviorSubject<readonly RemoteTrack[]>([]);
  private readonly errorsSubject = new Subject<Error>();
  private readonly messagesSubject = new Subject<MediaMessage>();
  readonly state$ = this.stateSubject.asObservable();
  readonly peers$ = this.peersSubject.asObservable();
  readonly tracks$ = this.tracksSubject.asObservable();
  readonly errors$ = this.errorsSubject.asObservable();
  readonly messages$ = this.messagesSubject.asObservable();

  private readonly sessions = new Map<string, PeerSession>();
  private readonly publications = new Map<string, Publication>();
  private signalingSubscription?: Subscription;
  private heartbeatSubscription?: Subscription;
  private cancelJoin?: () => void;
  private generation = 0;
  private selfId = "";
  private destroyed = false;
  private leaveTask?: Promise<void>;
  private destroyTask?: Promise<void>;

  constructor(private readonly dependencies: MediaClientDependencies) {}

  async join(options: JoinOptions): Promise<void> {
    this.assertAlive();
    if (this.stateSubject.value !== "idle")
      throw new Error("Leave the current session before joining");
    if (!options.roomId || !options.roomToken || !options.deviceToken)
      throw new Error("Room and device credentials are required");
    const generation = ++this.generation;
    this.stateSubject.next("joining");
    if (generation !== this.generation || this.destroyed) throw new Error("Join cancelled");
    this.signalingSubscription = this.dependencies.signaling.events$.subscribe((event) => {
      if (generation === this.generation) this.handleSignaling(event);
    });
    const cancelled = new Promise<never>((_, reject) => {
      this.cancelJoin = () => reject(new Error("Join cancelled"));
    });
    try {
      const joined = await Promise.race([this.dependencies.signaling.connect(options), cancelled]);
      if (generation !== this.generation || this.destroyed) throw new Error("Join cancelled");
      if (joined.peers.length > 3 || !joined.peerId) throw new Error("Invalid room membership");
      this.cancelJoin = undefined;
      this.selfId = joined.peerId;
      this.stateSubject.next("joined");
      if (generation !== this.generation || this.destroyed) throw new Error("Join cancelled");
      this.heartbeatSubscription = interval(20_000).subscribe(() => {
        if (generation === this.generation) this.safely(() => this.dependencies.signaling.ping());
      });
      for (const peerId of joined.peers) {
        if (generation !== this.generation || this.destroyed) throw new Error("Join cancelled");
        this.addPeer(peerId);
      }
    } catch (error) {
      if (generation === this.generation) {
        this.report(error);
        await this.leave();
      }
      throw error;
    }
  }

  async publish(publication: Publication): Promise<void> {
    this.assertJoined();
    if (!publication.id || this.publications.has(publication.id))
      throw new Error("Publication ID must be unique and non-empty");
    if ((publication.kind === "microphone") !== (publication.track.kind === "audio"))
      throw new Error("Publication kind does not match track kind");
    if ([...this.publications.values()].some((value) => value.track.id === publication.track.id))
      throw new Error("Track is already published");
    const attached: PeerSession[] = [];
    try {
      for (const session of this.sessions.values()) {
        attached.push(session);
        session.port.addTrack(publication.id, publication.track);
      }
      // Ownership transfers only after every synchronous attachment succeeds.
      this.publications.set(publication.id, publication);
    } catch (error) {
      for (const session of attached) {
        this.safely(() => session.port.removeTrack(publication.id));
        this.safely(() =>
          this.dependencies.signaling.send(session.id, {
            type: "track-removed",
            id: publication.id,
          }),
        );
      }
      throw error;
    }
  }

  async unpublish(id: string): Promise<void> {
    this.assertAlive();
    const publication = this.publications.get(id);
    if (!publication) return;
    this.publications.delete(id);
    for (const session of this.sessions.values()) {
      this.safely(() => session.port.removeTrack(id));
      this.safely(() =>
        this.dependencies.signaling.send(session.id, { type: "track-removed", id }),
      );
    }
    this.safely(() => publication.track.stop());
  }

  send(data: string): void {
    this.assertJoined();
    if (data.length > 16_384) throw new Error("Message exceeds 16384 characters");
    for (const session of this.sessions.values()) this.safely(() => session.port.send(data));
  }

  leave(): Promise<void> {
    if (this.leaveTask) return this.leaveTask;
    if (this.stateSubject.value === "idle" || this.stateSubject.value === "destroyed")
      return Promise.resolve();
    let complete!: () => void;
    const completion = new Promise<void>((resolve) => {
      complete = resolve;
    });
    // Install the guard before notifications: observers may synchronously request teardown.
    this.leaveTask = completion;
    ++this.generation;
    this.stateSubject.next("leaving");
    this.cancelJoin?.();
    this.cancelJoin = undefined;
    this.signalingSubscription?.unsubscribe();
    this.signalingSubscription = undefined;
    this.heartbeatSubscription?.unsubscribe();
    this.heartbeatSubscription = undefined;
    this.safely(() => this.dependencies.signaling.close());
    for (const session of this.sessions.values()) this.disposePeer(session);
    this.sessions.clear();
    for (const publication of this.publications.values())
      this.safely(() => publication.track.stop());
    this.publications.clear();
    this.selfId = "";
    this.peersSubject.next([]);
    this.tracksSubject.next([]);
    this.leaveTask = undefined;
    this.stateSubject.next(this.destroyed ? "destroyed" : "idle");
    complete();
    return completion;
  }

  destroy(): Promise<void> {
    if (this.destroyTask) return this.destroyTask;
    let complete!: () => void;
    this.destroyTask = new Promise<void>((resolve) => {
      complete = resolve;
    });
    this.destroyed = true;
    void this.leave().then(() => {
      if (this.stateSubject.value !== "destroyed") this.stateSubject.next("destroyed");
      this.stateSubject.complete();
      this.peersSubject.complete();
      this.tracksSubject.complete();
      this.errorsSubject.complete();
      this.messagesSubject.complete();
      complete();
    });
    return this.destroyTask;
  }

  private handleSignaling(event: SignalingEvent): void {
    switch (event.type) {
      case "peer-joined":
        if (this.stateSubject.value === "joined") {
          try {
            this.addPeer(event.peerId);
          } catch (error) {
            this.report(error);
          }
        }
        break;
      case "peer-left":
        this.removePeer(event.peerId);
        break;
      case "signal": {
        const session = this.sessions.get(event.from);
        if (session) this.enqueue(session, () => this.receiveSignal(session, event.payload));
        break;
      }
      case "error":
        this.report(event.error);
        break;
      case "closed":
        this.report(new Error("Signaling connection closed"));
        void this.leave();
        break;
    }
  }

  private addPeer(id: string): void {
    if (id === this.selfId || this.sessions.has(id)) return;
    if (this.sessions.size >= 3) throw new Error("Room capacity exceeded");
    const session: PeerSession = {
      id,
      port: this.dependencies.peers.create(id),
      generation: this.generation,
      polite: this.selfId > id,
      queue: Promise.resolve(),
      closed: false,
      ignoreOffer: false,
      negotiationRequested: false,
      earlyIce: [],
      metadata: new Map(),
      incoming: new Map(),
    };
    this.sessions.set(id, session);
    session.subscription = session.port.events$.subscribe((event) => {
      if (!this.active(session)) return;
      switch (event.type) {
        case "negotiation-needed":
          session.negotiationRequested = true;
          this.enqueue(session, () => this.negotiate(session));
          break;
        case "ice":
          this.safely(() =>
            this.dependencies.signaling.send(id, { type: "ice", candidate: event.candidate }),
          );
          break;
        case "track":
          session.incoming.set(event.mid, event.track);
          this.emitTracks();
          break;
        case "track-ended":
          session.incoming.delete(event.mid);
          this.emitTracks();
          break;
        case "message":
          this.messagesSubject.next({ peerId: id, data: event.data });
          break;
        case "failed":
          this.report(event.error);
          this.removePeer(id);
          break;
      }
    });
    try {
      for (const publication of this.publications.values()) {
        session.port.addTrack(publication.id, publication.track);
      }
      if (this.selfId < id) session.port.createDataChannel();
      this.peersSubject.next([...this.sessions.keys()]);
    } catch (error) {
      this.removePeer(id);
      throw error;
    }
  }

  private enqueue(session: PeerSession, operation: () => Promise<void>): void {
    session.queue = session.queue
      .then(async () => {
        if (this.active(session)) await operation();
      })
      .catch((error: unknown) => {
        if (this.active(session)) this.report(error);
      });
  }

  private async negotiate(session: PeerSession): Promise<void> {
    if (
      !this.active(session) ||
      !session.negotiationRequested ||
      session.port.signalingState !== "stable"
    )
      return;
    session.negotiationRequested = false;
    const description = await session.port.createOffer();
    if (!this.active(session)) return;
    await session.port.setLocalDescription(description);
    if (this.active(session)) {
      this.sendMetadata(session);
      this.dependencies.signaling.send(session.id, { type: "description", description });
    }
  }

  private async receiveSignal(session: PeerSession, payload: SignalPayload): Promise<void> {
    switch (payload.type) {
      case "description": {
        const description = payload.description;
        const collision = description.type === "offer" && session.port.signalingState !== "stable";
        session.ignoreOffer = !session.polite && collision;
        if (session.ignoreOffer) {
          session.earlyIce.length = 0;
          return;
        }
        if (collision) {
          await session.port.setLocalDescription({ type: "rollback" });
          if (!this.active(session)) return;
        }
        await session.port.setRemoteDescription(description);
        if (!this.active(session)) return;
        for (const candidate of session.earlyIce.splice(0)) {
          await session.port.addIceCandidate(candidate);
          if (!this.active(session)) return;
        }
        if (description.type === "offer") {
          const answer = await session.port.createAnswer();
          if (!this.active(session)) return;
          await session.port.setLocalDescription(answer);
          if (!this.active(session)) return;
          this.sendMetadata(session);
          this.dependencies.signaling.send(session.id, {
            type: "description",
            description: answer,
          });
        }
        await this.negotiate(session);
        break;
      }
      case "ice":
        if (session.ignoreOffer) return;
        if (!session.port.hasRemoteDescription) {
          if (session.earlyIce.length >= 128) throw new Error("Too many pending ICE candidates");
          session.earlyIce.push(payload.candidate);
        } else await session.port.addIceCandidate(payload.candidate);
        break;
      case "track":
        session.metadata.set(payload.id, { kind: payload.kind, mid: payload.mid });
        this.emitTracks();
        break;
      case "track-removed": {
        session.metadata.delete(payload.id);
        // Receiver tracks belong to the peer, not a publication: a later source can reuse
        // the same transceiver. Stopping here would permanently end that playback handle.
        this.emitTracks();
        break;
      }
    }
  }

  private sendMetadata(session: PeerSession): void {
    for (const binding of session.port.trackBindings()) {
      const publication = this.publications.get(binding.id);
      if (!publication) continue;
      this.dependencies.signaling.send(session.id, {
        type: "track",
        id: publication.id,
        kind: publication.kind,
        mid: binding.mid,
      });
    }
  }

  private emitTracks(): void {
    const tracks: RemoteTrack[] = [];
    for (const session of this.sessions.values()) {
      for (const [id, metadata] of session.metadata) {
        const track = session.incoming.get(metadata.mid);
        if (track) tracks.push({ peerId: session.id, id, kind: metadata.kind, track });
      }
    }
    this.tracksSubject.next(tracks);
  }

  private removePeer(id: string): void {
    const session = this.sessions.get(id);
    if (!session) return;
    this.disposePeer(session);
    this.sessions.delete(id);
    this.peersSubject.next([...this.sessions.keys()]);
    this.emitTracks();
  }

  private disposePeer(session: PeerSession): void {
    session.closed = true;
    session.subscription?.unsubscribe();
    this.safely(() => session.port.close());
    for (const track of session.incoming.values()) this.safely(() => track.stop());
    session.incoming.clear();
    session.metadata.clear();
    session.earlyIce.length = 0;
  }

  private active(session: PeerSession): boolean {
    return !session.closed && session.generation === this.generation && !this.destroyed;
  }
  private report(error: unknown): void {
    this.errorsSubject.next(error instanceof Error ? error : new Error(String(error)));
  }
  private safely(operation: () => void): void {
    try {
      operation();
    } catch (error) {
      this.report(error);
    }
  }
  private assertAlive(): void {
    if (this.destroyed) throw new Error("Media client is destroyed");
  }
  private assertJoined(): void {
    this.assertAlive();
    if (this.stateSubject.value !== "joined") throw new Error("Join a room first");
  }
}
