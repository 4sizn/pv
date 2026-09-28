import type {
  NativeDataSendResult,
  NativeMediaClientStreams,
  NativeMediaJoinOptions,
} from "@parentview/media-sdk/native";
import {
  filter,
  firstValueFrom,
  from,
  fromEvent,
  type Observable,
  Subscription,
  takeUntil,
  timeout,
} from "rxjs";
import { parseOrigin, type RoomInvitation } from "./invitation";
import type { DeviceIdentity } from "./laboratory-api";

export type NativeProbeStatus = "running" | "success" | "error";

export interface NativeProbeView {
  readonly origin: string;
  bindNativeProbe(action: () => void): void;
  setNativeProbe(status: NativeProbeStatus, message: string): void;
}

export interface NativeProbeClient
  extends Pick<NativeMediaClientStreams, "readyPeers$" | "messages$" | "errors$"> {
  join(options: NativeMediaJoinOptions): Promise<void>;
  send(data: string): Promise<NativeDataSendResult>;
  leave(): Promise<void>;
  destroy(): Promise<void>;
}

export interface NativeProbeApi {
  identity(origin: string, signal: AbortSignal): Promise<DeviceIdentity>;
  createRoom(
    origin: string,
    identity: DeviceIdentity,
    signal: AbortSignal,
  ): Promise<RoomInvitation>;
  clear(): void;
}

export interface NativeProbeDependencies {
  createClient(): Promise<NativeProbeClient>;
  createApi(): NativeProbeApi;
  timeoutMs?: number;
}

/**
 * Owns one developer-check workflow, its two clients, API identities and Rx waits.
 * Native readiness and delivery come only from the public SDK; no negotiation or retry policy.
 * destroy cancels waits and awaits cleanup, including a client whose creation finishes late.
 */
export class NativeDataProbe {
  private operation: Promise<void> | undefined;
  private abort: AbortController | undefined;
  private destroyed = false;

  constructor(
    private readonly view: NativeProbeView,
    private readonly dependencies: NativeProbeDependencies,
  ) {
    view.bindNativeProbe(() => void this.run());
  }

  run(): Promise<void> {
    if (this.operation) return this.operation;
    if (this.destroyed) return Promise.resolve();
    const abort = new AbortController();
    this.abort = abort;
    // Install ownership before calling injected code, including view handlers.
    const operation = Promise.resolve()
      .then(() => this.execute(abort))
      .finally(() => {
        this.operation = undefined;
        this.abort = undefined;
      });
    this.operation = operation;
    return operation;
  }

  destroy(): Promise<void> {
    this.destroyed = true;
    this.abort?.abort();
    return this.operation ?? Promise.resolve();
  }

  private async execute(abort: AbortController): Promise<void> {
    const { signal } = abort;
    const clients: NativeProbeClient[] = [];
    const apis: NativeProbeApi[] = [];
    const subscriptions = new Subscription();
    let failure = "서버 주소와 기기 등록을 확인해 주세요.";
    let completed = false;
    let cleanupFailed = false;
    try {
      signal.throwIfAborted();
      this.view.setNativeProbe("running", "두 네이티브 피어를 등록하고 있습니다…");
      const origin = parseOrigin(this.view.origin);
      // Each API caches identity per origin, so one instance per peer is required.
      for (let index = 0; index < 2; index += 1) apis.push(this.dependencies.createApi());
      const registrations = await Promise.allSettled(
        apis.map(async (api) => {
          try {
            return await api.identity(origin, signal);
          } catch (error) {
            abort.abort();
            throw error;
          }
        }),
      );
      signal.throwIfAborted();
      const identities = registrations.map((result) => {
        if (result.status === "rejected") throw result.reason;
        return result.value;
      });
      if (identities[0].peerId === identities[1].peerId) throw new Error("Duplicate identity");
      const room = await apis[0].createRoom(origin, identities[0], signal);
      signal.throwIfAborted();
      failure = "네이티브 클라이언트를 시작하지 못했습니다.";
      for (let index = 0; index < 2; index += 1) {
        signal.throwIfAborted();
        // Do not race creation against cancellation: a late handle must still be released.
        const client = await this.dependencies.createClient();
        clients.push(client);
        signal.throwIfAborted();
        subscriptions.add(client.errors$.subscribe(() => abort.abort()));
      }
      failure = "네이티브 피어 연결을 완료하지 못했습니다.";
      this.view.setNativeProbe("running", "네이티브 데이터 채널이 준비되기를 기다립니다…");
      await Promise.all(
        clients.map((client, index) =>
          this.wait(
            from(
              client.join({
                signalingUrl: `${origin.replace(/^http/, "ws")}/ws`,
                roomId: room.roomId,
                roomToken: room.roomToken,
                deviceToken: identities[index].deviceToken,
                iceServers: room.iceServers.map((server) => ({
                  urls: typeof server.urls === "string" ? [server.urls] : server.urls,
                  ...(server.username === undefined ? {} : { username: server.username }),
                  ...(server.credential === undefined ? {} : { credential: server.credential }),
                })),
              }),
            ),
            signal,
          ),
        ),
      );
      await Promise.all(
        clients.map((client, index) =>
          this.wait(
            client.readyPeers$.pipe(
              filter((peers) => peers.includes(identities[1 - index].peerId)),
            ),
            signal,
          ),
        ),
      );
      failure = "양방향 네이티브 메시지를 확인하지 못했습니다.";
      this.view.setNativeProbe("running", "양방향 메시지 수신을 확인하고 있습니다…");
      const messages = [
        `native-probe:${crypto.randomUUID()}:a`,
        `native-probe:${crypto.randomUUID()}:b`,
      ];
      // Subscribe before sending: delivery can occur before the send command resolves.
      const received = clients.map((client, index) =>
        this.wait(
          client.messages$.pipe(
            filter(
              (message) =>
                message.peerId === identities[1 - index].peerId &&
                message.data === messages[1 - index],
            ),
          ),
          signal,
        ),
      );
      await Promise.all([
        ...received,
        ...clients.map(async (client, index) => {
          const result = await this.wait(from(client.send(messages[index])), signal);
          if (
            result.failures.length ||
            result.acceptedPeerIds.length !== 1 ||
            result.acceptedPeerIds[0] !== identities[1 - index].peerId
          ) {
            throw new Error("Native send was not accepted by the expected peer");
          }
        }),
      ]);
      signal.throwIfAborted();
      completed = true;
      this.view.setNativeProbe("running", "메시지 확인 완료 · 네이티브 자원을 정리하고 있습니다…");
    } catch {
      // Render only fixed messages, never host errors, room secrets or ICE credentials.
    } finally {
      abort.abort();
      subscriptions.unsubscribe();
      const cleanup = await Promise.all(
        clients.map(async (client) => {
          let failed = false;
          try {
            await client.leave();
          } catch {
            failed = true;
          }
          try {
            await client.destroy();
          } catch {
            failed = true;
          }
          return failed;
        }),
      );
      cleanupFailed = cleanup.some(Boolean);
      for (const api of apis) {
        try {
          api.clear();
        } catch {
          cleanupFailed = true;
        }
      }
    }
    if (this.destroyed) return;
    if (completed && !cleanupFailed) {
      this.view.setNativeProbe("success", "네이티브 2피어 양방향 데이터 확인 · 자원 해제 완료");
    } else {
      this.view.setNativeProbe(
        "error",
        cleanupFailed
          ? "네이티브 자원 정리를 확인하지 못했습니다. 다시 시도해 주세요."
          : `네이티브 검사 실패 · ${failure} 다시 시도해 주세요.`,
      );
    }
  }

  private async wait<T>(source: Observable<T>, signal: AbortSignal): Promise<T> {
    signal.throwIfAborted();
    return firstValueFrom(
      source.pipe(
        timeout({ first: this.dependencies.timeoutMs ?? 15_000 }),
        takeUntil(fromEvent(signal, "abort")),
      ),
    );
  }
}
