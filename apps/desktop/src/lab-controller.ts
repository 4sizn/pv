import type { MediaClient, MediaTrackPort, SourceKind } from "@parentview/media-sdk";
import { Subscription } from "rxjs";
import type { CaptureKind, CapturePort } from "./browser-capture";
import {
  createInvitationLink,
  parseInvitation,
  parseOrigin,
  type RoomInvitation,
} from "./invitation";
import type { LabView } from "./lab-view";
import type { DeviceIdentity, LaboratoryApi } from "./laboratory-api";

interface LocalPreview {
  ids: string[];
  releaseListeners(): void;
}

interface LaboratorySession {
  client: MediaClient;
  abort: AbortController;
  subscriptions: Subscription;
  previews: Map<CaptureKind, LocalPreview>;
}

export interface LabDependencies {
  createClient(room: RoomInvitation): MediaClient;
  capture: CapturePort;
  adaptTrack(track: MediaStreamTrack): MediaTrackPort;
}

/** Forwards user intents; all connection and media lifecycle policy remains in the SDK. */
export class LabController {
  private session: LaboratorySession | undefined;
  private readonly lifetime = new AbortController();
  private operation: AbortController | undefined;
  private invitationLink = "";

  constructor(
    private readonly view: LabView,
    private readonly api: LaboratoryApi,
    private readonly dependencies: LabDependencies,
  ) {
    view.bind({
      create: () => void this.run((signal) => this.create(signal)),
      join: () => void this.run((signal) => this.join(signal)),
      leave: () => void this.leave().catch((error: unknown) => view.showError(error)),
      camera: () => void this.run(() => this.toggleCapture("camera")),
      screen: () => void this.run(() => this.toggleCapture("screen")),
      copy: () => void this.copyInvitation(),
      send: () => this.send(),
    });
    view.setCapabilities(dependencies.capture.screenAvailable);
  }

  async leave(): Promise<void> {
    this.operation?.abort();
    this.operation = undefined;
    this.view.setBusy(false);
    await this.closeSession();
  }

  private async closeSession(): Promise<void> {
    const session = this.session;
    if (!session) return;
    this.session = undefined;
    session.abort.abort();
    for (const preview of session.previews.values()) preview.releaseListeners();
    session.previews.clear();
    try {
      await session.client.destroy();
    } finally {
      session.subscriptions.unsubscribe();
      this.invitationLink = "";
      this.view.clearRoom();
      this.view.showNotice("실험실 연결과 미디어 전송을 종료했습니다.");
    }
  }

  async destroy(): Promise<void> {
    if (this.lifetime.signal.aborted) return;
    this.lifetime.abort();
    try {
      await this.leave();
    } finally {
      this.api.clear();
      this.view.destroy();
    }
  }

  private async create(signal: AbortSignal): Promise<void> {
    const origin = parseOrigin(this.view.origin);
    const identity = await this.api.identity(origin, signal);
    const room = await this.api.createRoom(origin, identity, signal);
    signal.throwIfAborted();
    await this.enter(room, identity);
  }

  private async join(signal: AbortSignal): Promise<void> {
    const room = parseInvitation(this.view.invitation);
    const identity = await this.api.identity(room.signalingOrigin, signal);
    signal.throwIfAborted();
    await this.enter(room, identity);
  }

  private async enter(room: RoomInvitation, identity: DeviceIdentity): Promise<void> {
    if (this.session) throw new Error("현재 실험실에서 나간 뒤 다시 참가해 주세요.");
    this.lifetime.signal.throwIfAborted();
    const session: LaboratorySession = {
      client: this.dependencies.createClient(room),
      abort: new AbortController(),
      subscriptions: new Subscription(),
      previews: new Map(),
    };
    this.session = session;
    this.invitationLink = createInvitationLink(room);
    this.view.setRoom(
      identity.peerId,
      room.roomId,
      room.signalingOrigin,
      room.iceServers.length,
      this.invitationLink,
    );
    session.subscriptions.add(
      session.client.state$.subscribe((state) => this.view.setState(state)),
    );
    session.subscriptions.add(
      session.client.peers$.subscribe((peers) => this.view.setPeers(peers)),
    );
    session.subscriptions.add(
      session.client.tracks$.subscribe((tracks) => this.view.setTracks(tracks)),
    );
    session.subscriptions.add(
      session.client.errors$.subscribe((error) => this.view.showError(error)),
    );
    session.subscriptions.add(
      session.client.messages$.subscribe((message) =>
        this.view.addMessage(message.peerId, message.data),
      ),
    );
    try {
      await session.client.join({
        roomId: room.roomId,
        roomToken: room.roomToken,
        deviceToken: identity.deviceToken,
      });
      if (this.session === session) {
        this.view.setIncomingInvitation("");
        this.view.showNotice("실험실에 참가했습니다. 초대 링크로 다른 탭을 연결해 보세요.");
      }
    } catch (error) {
      if (this.session === session) await this.closeSession();
      throw error;
    }
  }

  private async toggleCapture(kind: CaptureKind): Promise<void> {
    const session = this.session;
    if (!session) throw new Error("먼저 실험실에 참가해 주세요.");
    if (session.previews.has(kind)) {
      await this.stopCapture(session, kind);
      return;
    }
    const stream = await this.dependencies.capture.capture(kind, session.abort.signal);
    const tracks = stream.getTracks();
    const published: string[] = [];
    const transferred = new Set<MediaStreamTrack>();
    try {
      for (const track of tracks) {
        session.abort.signal.throwIfAborted();
        const id = `${kind}-${crypto.randomUUID()}`;
        const source: SourceKind = track.kind === "audio" ? "microphone" : kind;
        await session.client.publish({
          id,
          kind: source,
          track: this.dependencies.adaptTrack(track),
        });
        transferred.add(track);
        published.push(id);
      }
      session.abort.signal.throwIfAborted();
      const ended = () => {
        void this.stopCapture(session, kind).catch((error: unknown) => this.view.showError(error));
      };
      for (const track of tracks) track.addEventListener("ended", ended, { once: true });
      session.previews.set(kind, {
        ids: published,
        releaseListeners: () => {
          for (const track of tracks) track.removeEventListener("ended", ended);
        },
      });
      this.view.setLocal(kind, stream);
    } catch (error) {
      await Promise.allSettled(published.map((id) => session.client.unpublish(id)));
      for (const track of tracks) if (!transferred.has(track)) track.stop();
      throw error;
    }
  }

  private async stopCapture(session: LaboratorySession, kind: CaptureKind): Promise<void> {
    const preview = session.previews.get(kind);
    if (!preview) return;
    session.previews.delete(kind);
    preview.releaseListeners();
    if (this.session === session) this.view.setLocal(kind, null);
    const results = await Promise.allSettled(preview.ids.map((id) => session.client.unpublish(id)));
    const failure = results.find((result) => result.status === "rejected");
    if (failure?.status === "rejected") throw failure.reason;
  }

  private send(): void {
    this.view.clearError();
    const data = this.view.message;
    if (!data) return;
    try {
      if (!this.session) throw new Error("먼저 실험실에 참가해 주세요.");
      this.session.client.send(data);
      this.view.addMessage("", data, true);
    } catch (error) {
      this.view.showError(error);
    }
  }

  private async copyInvitation(): Promise<void> {
    if (!this.invitationLink) return;
    try {
      await navigator.clipboard.writeText(this.invitationLink);
      this.view.showNotice("초대 링크를 복사했습니다.");
    } catch {
      const field = this.view.element<HTMLTextAreaElement>("invitation-output");
      field.focus();
      field.select();
      this.view.showNotice("자동 복사가 제한되었습니다. 선택된 링크를 직접 복사해 주세요.");
    }
  }

  private async run(action: (signal: AbortSignal) => Promise<void>): Promise<void> {
    if (this.operation || this.lifetime.signal.aborted) return;
    const operation = new AbortController();
    this.operation = operation;
    this.view.clearError();
    this.view.setBusy(true);
    try {
      await action(operation.signal);
    } catch (error) {
      if (!operation.signal.aborted && !this.lifetime.signal.aborted) this.view.showError(error);
    } finally {
      if (this.operation === operation) {
        this.operation = undefined;
        this.view.setBusy(false);
      }
    }
  }
}
