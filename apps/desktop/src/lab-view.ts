import type { MediaState, RemoteTrack } from "@parentview/media-sdk";
import type { CaptureKind } from "./browser-capture";
import type { NativeProbeStatus } from "./native-data-probe";

export interface LabActions {
  create(): void;
  join(): void;
  leave(): void;
  camera(): void;
  screen(): void;
  copy(): void;
  send(): void;
}

const stateLabels: Record<MediaState, string> = {
  idle: "연결 전",
  joining: "참가 중",
  joined: "참가 완료",
  leaving: "연결 종료 중",
  destroyed: "연결 전",
};

const template = `
  <div class="lab-shell">
    <header class="topbar">
      <a class="brand" href="/" aria-label="ParentView 실험실 홈">
        <span class="brand-symbol" aria-hidden="true">p<span>v</span></span>
        <span>ParentView<span class="brand-divider">/</span><small>engineering</small></span>
      </a>
      <span class="build-label">DEVELOPER PREVIEW <span>v0.1</span></span>
    </header>

    <main>
      <section class="intro" aria-labelledby="page-title">
        <div><p class="eyebrow">REAL-TIME MEDIA WORKSPACE</p>
          <h1 id="page-title">Mesh laboratory<span class="title-dot">.</span></h1>
          <p class="intro-copy">카메라, 화면, 메시지. 최대 네 기기 사이의 실제 연결을 확인하세요.</p>
        </div>
        <div class="runtime-badge"><span class="status-dot"></span><span id="runtime-label">브라우저 어댑터</span><small>WebRTC mesh</small></div>
      </section>

      <aside class="scope-note" aria-label="구현 범위">
        <span class="scope-symbol" aria-hidden="true">i</span>
        <p><strong>개발자 검증용 실험실입니다.</strong> 완성된 ParentView 서비스 화면이 아닙니다. <span id="native-status">네이티브 미디어 · 원격 입력은 아직 구현되지 않았습니다.</span></p>
      </aside>

      <div id="error-panel" class="error-panel" role="alert" hidden><strong>진행할 수 없습니다</strong><span id="error-text"></span><button id="dismiss-error" class="text-button" type="button" aria-label="오류 알림 닫기">닫기</button></div>

      <div class="workspace">
        <aside class="session-panel" aria-labelledby="session-title">
          <div class="section-heading"><h2 id="session-title">실험실 연결</h2><span class="section-code">SESSION</span></div>
          <label class="field-label" for="server-origin">시그널링 서버</label>
          <input id="server-origin" type="url" value="http://localhost:8787" autocomplete="off" spellcheck="false" />
          <p class="field-help">로컬 서버를 먼저 실행해 주세요.</p>
          <button id="create-room" class="button primary" type="button" data-testid="create-room"><span>새 실험실 열기</span><span aria-hidden="true">↗</span></button>
          <div class="or-divider"><span>초대받았다면</span></div>
          <label class="field-label" for="invitation-input">초대 링크</label>
          <textarea id="invitation-input" rows="3" placeholder="초대 링크 전체를 붙여 넣으세요" autocomplete="off" spellcheck="false" data-testid="invitation-input"></textarea>
          <button id="join-room" class="button secondary" type="button" data-testid="join-room">초대로 참가</button>

          <section id="share-section" class="share-section" aria-label="실험실 초대" hidden>
            <label class="field-label" for="invitation-output">이 실험실로 초대하기</label>
            <textarea id="invitation-output" rows="2" readonly spellcheck="false" data-testid="invitation-output"></textarea>
            <button id="copy-invitation" class="button secondary" type="button">초대 링크 복사 <span aria-hidden="true">⧉</span></button>
            <p class="field-help">링크를 가진 기기가 참가할 수 있습니다. 필요한 사람에게만 공유하세요.</p>
          </section>

          <div class="session-details">
            <div><span>내 기기</span><code id="identity-label">아직 등록되지 않음</code></div>
            <div><span>실험실</span><code id="room-label">—</code></div>
            <div><span>ICE 서버</span><span id="ice-label">연결 후 확인</span></div>
          </div>
          <button id="leave-room" class="button leave" type="button" disabled data-testid="leave-room">실험실 나가기 <span aria-hidden="true">↗</span></button>
          <p class="privacy-note">기기 인증은 탭마다 새로 발급됩니다.<br />인증 정보는 이 탭의 메모리에만 보관합니다.</p>
        </aside>

        <div class="media-workspace">
          <section class="mesh-strip" aria-label="실시간 참가 상태">
            <div class="mesh-summary"><span class="field-label">참가 상태</span><strong id="session-state" data-state="idle" data-testid="media-state">연결 전</strong><span id="participant-count">0 / 4 기기</span></div>
            <div id="mesh-seats" class="mesh-seats"></div>
          </section>

          <section class="media-section" aria-labelledby="local-title">
            <div class="section-heading"><div class="heading-with-note"><h2 id="local-title">내 미디어</h2><span>이 탭에서 전송하는 영상</span></div><span class="source-tag">LOCAL</span></div>
            <div class="local-layout">
              <article id="local-camera-tile" class="video-tile local-camera">
                <video id="local-camera" autoplay playsinline muted hidden data-testid="local-video"></video>
                <div class="video-placeholder" id="camera-placeholder"><span class="camera-glyph" aria-hidden="true"></span><strong>카메라가 꺼져 있습니다</strong><p>실험실 참가 후 카메라와 마이크를 켜세요.</p></div>
                <div class="tile-caption"><span><i class="source-dot"></i>내 카메라</span><span id="camera-state">OFF</span></div>
              </article>
              <div class="capture-controls">
                <div class="control-description"><span class="control-icon" aria-hidden="true">◉</span><h3>내보낼 소스 선택</h3><p>각 기기가 카메라·마이크와 화면을 직접 전송합니다.</p></div>
                <button id="toggle-camera" class="button secondary" type="button" disabled data-testid="toggle-camera">카메라·마이크 켜기</button>
                <button id="toggle-screen" class="button secondary" type="button" disabled data-testid="toggle-screen">화면 공유하기 <span aria-hidden="true">↗</span></button>
                <p id="screen-support" class="field-help">화면 공유는 브라우저 지원 여부에 따라 달라집니다.</p>
                <div class="media-boundary"><span>전송 방식</span><strong>기기 간 직접 연결</strong><p>TURN 설정이 있으면 릴레이를 사용할 수 있습니다.</p></div>
              </div>
            </div>
            <article id="local-screen-tile" class="video-tile screen-tile" hidden><video id="local-screen" autoplay playsinline muted></video><div class="tile-caption"><span><i class="source-dot"></i>내 공유 화면</span><span>LIVE</span></div></article>
          </section>

          <section class="media-section remote-section" aria-labelledby="remote-title">
            <div class="section-heading"><div class="heading-with-note"><h2 id="remote-title">받는 미디어</h2><span id="remote-count">다른 기기를 기다립니다</span></div><span class="source-tag">REMOTE</span></div>
            <div id="remote-empty" class="remote-empty"><span class="connection-glyph" aria-hidden="true">↔</span><div><strong>다른 기기를 초대해 보세요</strong><p>새 탭이나 다른 기기에서 초대 링크로 참가한 뒤 미디어를 켜세요.</p></div></div>
            <div id="remote-tracks" class="remote-grid" data-testid="remote-tracks"></div>
          </section>

          <section class="data-section" aria-labelledby="data-title">
            <div class="section-heading"><div class="heading-with-note"><h2 id="data-title">데이터 채널</h2><span>텍스트로 경로 확인</span></div><span class="source-tag">DATA</span></div>
            <form id="message-form" class="message-form"><label class="sr-only" for="message-input">전송할 메시지</label><input id="message-input" placeholder="연결된 기기에 메시지 보내기" maxlength="4096" autocomplete="off" disabled data-testid="message-input" /><button id="send-message" class="button primary" type="submit" disabled data-testid="send-message">보내기 <span aria-hidden="true">↗</span></button></form>
            <ol id="message-log" class="message-log" aria-live="polite" aria-label="메시지 기록"><li class="log-empty">메시지를 보내면 송수신 기록이 여기에 표시됩니다.</li></ol>
          </section>
          <section id="native-probe-section" class="data-section" aria-labelledby="native-probe-title" hidden>
            <div class="section-heading"><div class="heading-with-note"><h2 id="native-probe-title">네이티브 데이터 검사</h2><span>이 호스트 안의 두 네이티브 피어</span></div><span class="source-tag">NATIVE · DATA ONLY</span></div>
            <p class="field-help">Rust WebRTC 엔진의 양방향 메시지와 자원 해제를 확인합니다. 네이티브 미디어 · 원격 입력 검사는 포함하지 않습니다.</p>
            <button id="native-probe-start" class="button secondary" type="button" data-testid="native-probe-start">네이티브 데이터 검사 시작</button>
            <p id="native-probe-status" class="field-help" role="status" aria-live="polite" data-testid="native-probe-status">검사 전 · 시그널링 서버가 실행 중이어야 합니다.</p>
          </section>
        </div>
      </div>
      <footer><span>ParentView Next <span class="footer-dot">·</span> Browser validation surface</span><span>최대 4기기 <span class="footer-dot">/</span> 역할 없는 미디어 SDK <span class="footer-dot">/</span> Native media pending</span></footer>
    </main>
    <div id="notice" class="notice" role="status" aria-live="polite"></div>
  </div>
`;

export class LabView {
  private readonly events = new AbortController();
  private readonly remoteElements = new Map<string, HTMLElement>();
  private readonly root: HTMLElement;
  private state: MediaState = "idle";
  private busy = false;
  private screenAvailable = false;
  private peers: readonly string[] = [];
  private ownId = "";
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    root: HTMLElement,
    private readonly playbackTrack: (track: RemoteTrack["track"]) => MediaStreamTrack,
  ) {
    this.root = root;
    root.innerHTML = template;
    this.renderSeats();
  }

  element<T extends HTMLElement>(id: string): T {
    const element = this.root.querySelector<T>(`#${id}`);
    if (!element) throw new Error(`Missing laboratory element: ${id}`);
    return element;
  }

  bind(actions: LabActions): void {
    const clicks: Record<string, () => void> = {
      "create-room": actions.create,
      "join-room": actions.join,
      "leave-room": actions.leave,
      "toggle-camera": actions.camera,
      "toggle-screen": actions.screen,
      "copy-invitation": actions.copy,
      "dismiss-error": () => this.clearError(),
    };
    for (const [id, action] of Object.entries(clicks)) {
      this.element(id).addEventListener("click", action, { signal: this.events.signal });
    }
    this.element("message-form").addEventListener(
      "submit",
      (event) => {
        event.preventDefault();
        actions.send();
      },
      { signal: this.events.signal },
    );
  }

  get origin(): string {
    return this.element<HTMLInputElement>("server-origin").value;
  }

  bindNativeProbe(action: () => void): void {
    this.element("native-probe-start").addEventListener("click", action, {
      signal: this.events.signal,
    });
  }

  enableNativeProbe(): void {
    if (this.events.signal.aborted) return;
    this.element("native-probe-section").hidden = false;
  }

  setNativeProbe(status: NativeProbeStatus, message: string): void {
    if (this.events.signal.aborted) return;
    const running = status === "running";
    const button = this.element<HTMLButtonElement>("native-probe-start");
    button.disabled = running;
    button.textContent = running ? "네이티브 데이터 검사 중…" : "네이티브 데이터 다시 검사";
    this.element("native-probe-section").setAttribute("aria-busy", String(running));
    const label = this.element("native-probe-status");
    label.dataset.state = status;
    label.textContent = message;
  }

  get invitation(): string {
    return this.element<HTMLTextAreaElement>("invitation-input").value;
  }

  get message(): string {
    return this.element<HTMLInputElement>("message-input").value.trim();
  }

  setIncomingInvitation(value: string): void {
    this.element<HTMLTextAreaElement>("invitation-input").value = value;
  }

  setCapabilities(screenAvailable: boolean): void {
    this.screenAvailable = screenAvailable;
    if (!screenAvailable) {
      this.element("screen-support").textContent =
        "이 환경에서는 화면 공유 API를 지원하지 않습니다.";
    }
    this.renderControls();
  }

  setRuntime(platform?: string): void {
    this.element("runtime-label").textContent = platform
      ? `Tauri 호스트 · ${platform}`
      : "브라우저 어댑터";
    if (platform) {
      this.element("native-status").textContent =
        "카메라·음성·화면 공유는 WebView 브라우저 어댑터로 검증합니다. 네이티브 미디어·원격 입력은 아직 지원하지 않습니다.";
    }
  }

  setRoom(peerId: string, roomId: string, origin: string, iceCount: number, link: string): void {
    this.ownId = peerId;
    this.element("identity-label").textContent = shortId(peerId);
    this.element("identity-label").title = peerId;
    this.element("room-label").textContent = shortId(roomId);
    this.element("room-label").title = roomId;
    this.element<HTMLInputElement>("server-origin").value = origin;
    this.element("ice-label").textContent = iceCount
      ? `${iceCount}개 설정됨`
      : "없음 · 로컬/LAN 검증";
    this.element<HTMLTextAreaElement>("invitation-output").value = link;
    this.element("share-section").hidden = false;
    this.renderSeats();
  }

  clearRoom(): void {
    this.element("room-label").textContent = "—";
    this.element("room-label").removeAttribute("title");
    this.element("ice-label").textContent = "연결 후 확인";
    this.element<HTMLTextAreaElement>("invitation-output").value = "";
    this.element("share-section").hidden = true;
    this.setPeers([]);
    this.setTracks([]);
    this.setLocal("camera", null);
    this.setLocal("screen", null);
    this.setState("idle");
  }

  setState(state: MediaState): void {
    this.state = state;
    const label = this.element("session-state");
    label.textContent = stateLabels[state];
    label.dataset.state = state;
    this.renderSeats();
    this.renderControls();
  }

  setBusy(busy: boolean): void {
    this.busy = busy;
    this.root.setAttribute("aria-busy", String(busy));
    this.renderControls();
  }

  setPeers(peers: readonly string[]): void {
    this.peers = peers;
    this.element("remote-count").textContent = peers.length
      ? `${peers.length}기기 참가 중 · 수신 트랙은 아래에 표시됩니다`
      : "다른 기기를 기다립니다";
    this.renderSeats();
    this.renderControls();
  }

  setLocal(kind: CaptureKind, stream: MediaStream | null): void {
    const video = this.element<HTMLVideoElement>(`local-${kind}`);
    video.srcObject = stream;
    if (stream) void video.play().catch(() => undefined);
    if (kind === "camera") {
      video.hidden = !stream;
      this.element("camera-placeholder").hidden = !!stream;
      this.element("camera-state").textContent = stream ? "LIVE · 음소거 미리보기" : "OFF";
      this.element("local-camera-tile").classList.toggle("has-stream", !!stream);
      this.element("toggle-camera").textContent = stream
        ? "카메라·마이크 끄기"
        : "카메라·마이크 켜기";
    } else {
      this.element("local-screen-tile").hidden = !stream;
      this.element("toggle-screen").textContent = stream ? "화면 공유 중지" : "화면 공유하기 ↗";
    }
  }

  setTracks(tracks: readonly RemoteTrack[]): void {
    const retained = new Set<string>();
    for (const track of tracks) {
      const key = `${track.peerId}:${track.id}`;
      retained.add(key);
      const nativeTrack = this.playbackTrack(track.track);
      const existing = this.remoteElements.get(key);
      if (existing) {
        const media = existing.querySelector("video, audio");
        if (
          media instanceof HTMLMediaElement &&
          (!(media.srcObject instanceof MediaStream) ||
            media.srcObject.getTracks()[0] !== nativeTrack)
        ) {
          media.srcObject = new MediaStream([nativeTrack]);
          void media.play().catch(() => {
            this.showNotice("자동 재생이 제한되었습니다. 수신 미디어의 재생 버튼을 눌러 주세요.");
          });
        }
        continue;
      }
      const card = document.createElement("article");
      card.className = `video-tile remote-tile ${nativeTrack.kind === "audio" ? "audio-tile" : ""}`;
      card.dataset.peerId = track.peerId;
      card.dataset.source = track.kind;
      const media = document.createElement(nativeTrack.kind === "video" ? "video" : "audio");
      media.autoplay = true;
      media.controls = true;
      media.srcObject = new MediaStream([nativeTrack]);
      if (media instanceof HTMLVideoElement) {
        media.playsInline = true;
        media.dataset.testid = "remote-video";
        if (track.kind === "screen") media.classList.add("screen-video");
      }
      const caption = document.createElement("div");
      caption.className = "tile-caption";
      const labels = { camera: "카메라", screen: "공유 화면", microphone: "마이크" };
      caption.textContent = `${shortId(track.peerId)} · ${labels[track.kind]}`;
      card.append(media, caption);
      this.element("remote-tracks").append(card);
      this.remoteElements.set(key, card);
      void media.play().catch(() => {
        this.showNotice("자동 재생이 제한되었습니다. 수신 미디어의 재생 버튼을 눌러 주세요.");
      });
    }
    for (const [key, card] of this.remoteElements) {
      if (retained.has(key)) continue;
      const media = card.querySelector("video, audio");
      if (media instanceof HTMLMediaElement) media.srcObject = null;
      card.remove();
      this.remoteElements.delete(key);
    }
    this.element("remote-empty").hidden = tracks.length > 0;
  }

  addMessage(peerId: string, data: string, outgoing = false): void {
    const log = this.element("message-log");
    log.querySelector(".log-empty")?.remove();
    const item = document.createElement("li");
    item.className = outgoing ? "message outgoing" : "message incoming";
    item.dataset.testid = outgoing ? "sent-message" : "received-message";
    const author = document.createElement("span");
    author.className = "message-author";
    author.textContent = outgoing ? "나 · 송신 요청" : shortId(peerId);
    const content = document.createElement("span");
    content.className = "message-content";
    content.textContent = data;
    const time = document.createElement("time");
    const now = new Date();
    time.dateTime = now.toISOString();
    time.textContent = now.toLocaleTimeString("ko-KR", { hour12: false });
    item.append(author, content, time);
    log.append(item);
    while (log.children.length > 100) log.firstElementChild?.remove();
    log.scrollTop = log.scrollHeight;
    if (outgoing) this.element<HTMLInputElement>("message-input").value = "";
  }

  showError(error: unknown): void {
    this.element("error-text").textContent =
      error instanceof Error ? error.message : "요청을 처리할 수 없습니다.";
    this.element("error-panel").hidden = false;
  }

  clearError(): void {
    this.element("error-panel").hidden = true;
    this.element("error-text").textContent = "";
  }

  showNotice(message: string): void {
    if (this.events.signal.aborted) return;
    clearTimeout(this.noticeTimer);
    this.element("notice").textContent = message;
    this.noticeTimer = setTimeout(() => {
      this.element("notice").textContent = "";
    }, 5_000);
  }

  destroy(): void {
    this.events.abort();
    clearTimeout(this.noticeTimer);
    this.setTracks([]);
    this.setLocal("camera", null);
    this.setLocal("screen", null);
  }

  private renderControls(): void {
    const idle = this.state === "idle" || this.state === "destroyed";
    for (const id of ["create-room", "join-room", "server-origin", "invitation-input"]) {
      this.element<HTMLInputElement>(id).disabled = !idle || this.busy;
    }
    for (const id of ["toggle-camera"]) {
      this.element<HTMLInputElement>(id).disabled = this.state !== "joined" || this.busy;
    }
    for (const id of ["message-input", "send-message"]) {
      this.element<HTMLInputElement>(id).disabled =
        this.state !== "joined" || this.busy || this.peers.length === 0;
    }
    this.element<HTMLButtonElement>("toggle-screen").disabled =
      this.state !== "joined" || this.busy || !this.screenAvailable;
    this.element<HTMLButtonElement>("leave-room").disabled = idle || this.state === "leaving";
  }

  private renderSeats(): void {
    const joined = this.state === "joined";
    this.element("participant-count").textContent =
      `${joined ? this.peers.length + 1 : 0} / 4 기기`;
    const seats = this.element("mesh-seats");
    seats.replaceChildren();
    const ids = joined ? [this.ownId, ...this.peers] : [];
    for (let index = 0; index < 4; index += 1) {
      const seat = document.createElement("div");
      seat.className = `mesh-seat ${ids[index] ? "occupied" : "vacant"} ${joined && index === 0 ? "self" : ""}`;
      const node = document.createElement("span");
      node.className = "mesh-node";
      node.textContent = ids[index] ? (index === 0 ? "나" : "↔") : "+";
      const label = document.createElement("span");
      label.textContent = ids[index] ? (index === 0 ? "이 탭" : shortId(ids[index])) : "빈자리";
      seat.append(node, label);
      seats.append(seat);
    }
  }
}

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}…` : id;
}
