import type { RoomInvitation } from "./invitation";

export interface DeviceIdentity {
  peerId: string;
  deviceToken: string;
}

/** In-memory, per-tab identity. The server alone issues and authenticates it. */
export class LaboratoryApi {
  private readonly identities = new Map<string, DeviceIdentity>();

  async identity(origin: string, signal: AbortSignal): Promise<DeviceIdentity> {
    const existing = this.identities.get(origin);
    if (existing) return existing;
    const result = await this.request(origin, "/devices", signal);
    if (typeof result.peerId !== "string" || typeof result.deviceToken !== "string") {
      throw new Error("서버의 기기 등록 응답이 올바르지 않습니다.");
    }
    const identity = { peerId: result.peerId, deviceToken: result.deviceToken };
    this.identities.set(origin, identity);
    return identity;
  }

  async createRoom(
    origin: string,
    identity: DeviceIdentity,
    signal: AbortSignal,
  ): Promise<RoomInvitation> {
    const result = await this.request(origin, "/rooms", signal, identity.deviceToken);
    if (
      typeof result.roomId !== "string" ||
      typeof result.roomToken !== "string" ||
      !Array.isArray(result.iceServers)
    ) {
      throw new Error("서버의 실험실 생성 응답이 올바르지 않습니다.");
    }
    return {
      version: 1,
      roomId: result.roomId,
      roomToken: result.roomToken,
      signalingOrigin: origin,
      iceServers: result.iceServers,
    };
  }

  clear(): void {
    this.identities.clear();
  }

  private async request(
    origin: string,
    path: string,
    signal: AbortSignal,
    token?: string,
  ): Promise<Record<string, unknown>> {
    let response: Response;
    try {
      response = await fetch(`${origin}${path}`, {
        method: "POST",
        headers: token ? { Authorization: `Bearer ${token}` } : {},
        signal: AbortSignal.any([signal, AbortSignal.timeout(15_000)]),
        credentials: "omit",
        cache: "no-store",
        referrerPolicy: "no-referrer",
      });
    } catch (error) {
      if (signal.aborted) throw error;
      throw new Error("시그널링 서버에 연결할 수 없습니다. 서버 실행 상태와 주소를 확인해 주세요.");
    }
    if (!response.ok) {
      throw new Error(`서버가 요청을 거절했습니다 (HTTP ${response.status}). 다시 연결해 주세요.`);
    }
    const result: unknown = await response.json();
    if (!result || typeof result !== "object") throw new Error("서버 응답을 읽을 수 없습니다.");
    return result as Record<string, unknown>;
  }
}
