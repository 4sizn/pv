export interface RoomInvitation {
  version: 1;
  roomId: string;
  roomToken: string;
  signalingOrigin: string;
  iceServers: RTCIceServer[];
}

export function parseOrigin(value: string): string {
  const url = new URL(value.trim());
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/"
  ) {
    throw new Error("시그널링 주소는 http 또는 https 서버의 기본 주소여야 합니다.");
  }
  return url.origin;
}

export function createInvitationLink(room: RoomInvitation): string {
  const url = new URL(window.location.href);
  url.search = "";
  url.hash = new URLSearchParams({ invite: JSON.stringify(room) }).toString();
  return url.toString();
}

export function parseInvitation(value: string): RoomInvitation {
  if (value.length > 32_768) throw new Error("초대 링크가 너무 깁니다.");
  let invitation: unknown;
  try {
    const fragment = new URL(value.trim()).hash.slice(1);
    invitation = JSON.parse(new URLSearchParams(fragment).get("invite") ?? "null");
  } catch {
    throw new Error("유효한 초대 링크 전체를 붙여 넣어 주세요.");
  }
  if (!invitation || typeof invitation !== "object") {
    throw new Error("초대 링크에서 참가 정보를 찾을 수 없습니다.");
  }
  const data = invitation as Record<string, unknown>;
  if (
    data.version !== 1 ||
    typeof data.roomId !== "string" ||
    !data.roomId ||
    typeof data.roomToken !== "string" ||
    !data.roomToken ||
    typeof data.signalingOrigin !== "string" ||
    !Array.isArray(data.iceServers)
  ) {
    throw new Error("초대 정보가 올바르지 않습니다. 새 초대 링크를 받아 주세요.");
  }
  const iceServers = data.iceServers.map((server: unknown): RTCIceServer => {
    if (!server || typeof server !== "object") throw new Error("ICE 설정이 잘못되었습니다.");
    const ice = server as Record<string, unknown>;
    const urls = typeof ice.urls === "string" ? [ice.urls] : ice.urls;
    if (
      !Array.isArray(urls) ||
      urls.length === 0 ||
      !urls.every(isIceUrl) ||
      (ice.username !== undefined && typeof ice.username !== "string") ||
      (ice.credential !== undefined && typeof ice.credential !== "string")
    ) {
      throw new Error("초대 링크의 ICE 설정이 올바르지 않습니다.");
    }
    if (
      urls.some((url) => /^turns?:/i.test(url)) &&
      (typeof ice.username !== "string" ||
        ice.username.length === 0 ||
        typeof ice.credential !== "string" ||
        ice.credential.length === 0)
    ) {
      throw new Error("TURN 설정에는 사용자 이름과 자격 증명이 필요합니다.");
    }
    return {
      urls,
      ...(typeof ice.username === "string" ? { username: ice.username } : {}),
      ...(typeof ice.credential === "string" ? { credential: ice.credential } : {}),
    };
  });
  return {
    version: 1,
    roomId: data.roomId,
    roomToken: data.roomToken,
    signalingOrigin: parseOrigin(data.signalingOrigin),
    iceServers,
  };
}

/** Basic ICE URI forms only; the browser remains authoritative for full WebRTC validation. */
function isIceUrl(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const match =
    /^([A-Za-z]+):(\[[0-9a-fA-F:.]+\]|[^:[\]/?#@\s]+)(?::(\d+))?(?:\?transport=(udp|tcp))?$/.exec(
      value,
    );
  if (!match) return false;
  const scheme = match[1]?.toLowerCase();
  const isTurn = scheme === "turn" || scheme === "turns";
  if (!isTurn && scheme !== "stun" && scheme !== "stuns") return false;
  if (!isTurn && match[4]) return false;
  const port = match[3];
  return port === undefined || (Number(port) > 0 && Number(port) <= 65_535);
}

/** Consume secrets before rendering. No invitation or credential is persisted. */
export function consumeLocationInvitation(): string {
  if (!new URLSearchParams(window.location.hash.slice(1)).has("invite")) return "";
  const link = window.location.href;
  window.history.replaceState(null, "", window.location.pathname);
  return link;
}
