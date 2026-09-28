import { mkdir, writeFile } from "node:fs/promises";
import { expect, type Page, test } from "@playwright/test";

async function joined(page: Page): Promise<void> {
  await expect(page.getByTestId("media-state")).toHaveAttribute("data-state", "joined");
}

async function installMediaDiagnostics(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const connections: RTCPeerConnection[] = [];
    const log: Record<string, unknown>[] = [];
    const target = window as Window & {
      __meshDebug?: { connections: RTCPeerConnection[]; log: Record<string, unknown>[] };
    };
    target.__meshDebug = { connections, log };
    const record = (event: Record<string, unknown>) =>
      log.push({ at: Math.round(performance.now()), ...event });
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class extends NativePeer {
      readonly debugId: number;
      constructor(configuration?: RTCConfiguration) {
        super(configuration);
        this.debugId = connections.length;
        connections.push(this);
        for (const type of [
          "connectionstatechange",
          "iceconnectionstatechange",
          "signalingstatechange",
          "negotiationneeded",
        ]) {
          this.addEventListener(type, () => this.note(type));
        }
        this.addEventListener("icecandidate", ({ candidate }) =>
          this.note("candidate", { candidateType: candidate?.type ?? "end" }),
        );
        this.addEventListener("track", ({ track, transceiver }) => {
          this.note("track", { trackId: track.id, kind: track.kind, mid: transceiver.mid });
          for (const type of ["ended", "mute", "unmute"])
            track.addEventListener(type, () =>
              this.note(`track:${type}`, { trackId: track.id, readyState: track.readyState }),
            );
        });
        this.addEventListener("datachannel", ({ channel }) => this.watchChannel(channel));
      }
      private note(event: string, extra: Record<string, unknown> = {}) {
        record({
          peer: this.debugId,
          event,
          signaling: this.signalingState,
          connection: this.connectionState,
          ice: this.iceConnectionState,
          ...extra,
        });
      }
      private watchChannel(channel: RTCDataChannel) {
        for (const event of ["open", "close", "error"])
          channel.addEventListener(event, (value) => {
            const error = (value as RTCErrorEvent).error;
            this.note(`channel:${event}`, {
              id: channel.id,
              readyState: channel.readyState,
              error: error?.name,
              detail: error?.errorDetail,
              cause: error?.sctpCauseCode,
            });
          });
      }
      override createDataChannel(label: string, options?: RTCDataChannelInit): RTCDataChannel {
        const channel = super.createDataChannel(label, options);
        this.watchChannel(channel);
        return channel;
      }
      override async setLocalDescription(
        description?: RTCLocalSessionDescriptionInit,
      ): Promise<void> {
        this.note("local:start", { type: description?.type });
        try {
          await super.setLocalDescription(description);
          this.note("local:end", { type: description?.type });
        } catch (error) {
          this.note("local:failed", { type: description?.type, error: (error as Error).name });
          throw error;
        }
      }
      override async setRemoteDescription(description: RTCSessionDescriptionInit): Promise<void> {
        this.note("remote:start", { type: description.type });
        try {
          await super.setRemoteDescription(description);
          this.note("remote:end", { type: description.type });
        } catch (error) {
          this.note("remote:failed", { type: description.type, error: (error as Error).name });
          throw error;
        }
      }
      override close(): void {
        this.note("close");
        super.close();
      }
    };
    const NativeSocket = window.WebSocket;
    window.WebSocket = class extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        this.addEventListener("message", ({ data }) => {
          try {
            const message = JSON.parse(data as string);
            record({
              event: "socket:receive",
              type: message.type,
              payloadType: message.payload?.type,
              descriptionType: message.payload?.description?.type,
              from: message.from,
              peerId: message.peerId,
              code: message.code,
            });
          } catch {
            /* Native parsing reports invalid data separately. */
          }
        });
        this.addEventListener("close", ({ code }) => record({ event: "socket:closed", code }));
      }
      override send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
        if (typeof data === "string") {
          try {
            const message = JSON.parse(data);
            record({
              event: "socket:send",
              type: message.type,
              payloadType: message.payload?.type,
              descriptionType: message.payload?.description?.type,
              to: message.to,
            });
          } catch {
            /* Do not copy raw messages into diagnostics. */
          }
        }
        super.send(data);
      }
    };
  });
}

async function mediaDiagnostics(page: Page): Promise<unknown> {
  return page.evaluate(async () => {
    const debug = (
      window as Window & {
        __meshDebug?: { connections: RTCPeerConnection[]; log: Record<string, unknown>[] };
      }
    ).__meshDebug;
    return {
      visibility: document.visibilityState,
      playback: [
        ...document.querySelectorAll<HTMLMediaElement>(
          "#remote-tracks video, #remote-tracks audio",
        ),
      ].map((element) => ({
        tag: element.tagName,
        tracks: (element.srcObject as MediaStream | null)
          ?.getTracks()
          .map((track) => ({ id: track.id, state: track.readyState, muted: track.muted })),
        readyState: element.readyState,
        paused: element.paused,
        frames:
          element instanceof HTMLVideoElement
            ? element.getVideoPlaybackQuality().totalVideoFrames
            : undefined,
      })),
      log: debug?.log,
      peers: await Promise.all(
        (debug?.connections ?? []).map(async (connection) => {
          const stats: Record<string, unknown>[] = [];
          (await connection.getStats()).forEach((report) => {
            if (
              [
                "inbound-rtp",
                "outbound-rtp",
                "transport",
                "data-channel",
                "media-source",
                "codec",
              ].includes(report.type)
            ) {
              stats.push({
                type: report.type,
                id: report.id,
                codecId: report.codecId,
                mimeType: report.mimeType,
                ssrc: report.ssrc,
                kind: report.kind,
                mid: report.mid,
                trackIdentifier: report.trackIdentifier,
                bytesReceived: report.bytesReceived,
                bytesSent: report.bytesSent,
                packetsReceived: report.packetsReceived,
                packetsSent: report.packetsSent,
                framesDecoded: report.framesDecoded,
                framesEncoded: report.framesEncoded,
                framesSent: report.framesSent,
                frames: report.frames,
                framesDropped: report.framesDropped,
                framesPerSecond: report.framesPerSecond,
                qualityLimitationReason: report.qualityLimitationReason,
                nackCount: report.nackCount,
                pliCount: report.pliCount,
                packetsLost: report.packetsLost,
                totalAudioEnergy: report.totalAudioEnergy,
                totalSamplesDuration: report.totalSamplesDuration,
                audioLevel: report.audioLevel,
                dtlsState: report.dtlsState,
                iceState: report.iceState,
                state: report.state,
                messagesReceived: report.messagesReceived,
              });
            }
          });
          return {
            connection: connection.connectionState,
            ice: connection.iceConnectionState,
            signaling: connection.signalingState,
            transceivers: connection.getTransceivers().map((transceiver) => ({
              mid: transceiver.mid,
              direction: transceiver.direction,
              currentDirection: transceiver.currentDirection,
              sender: transceiver.sender.track?.id,
              senderState: transceiver.sender.track?.readyState,
              senderMuted: transceiver.sender.track?.muted,
              senderEnabled: transceiver.sender.track?.enabled,
              senderEncodings: transceiver.sender.getParameters().encodings,
              senderSettings: transceiver.sender.track
                ? {
                    width: transceiver.sender.track.getSettings().width,
                    height: transceiver.sender.track.getSettings().height,
                    frameRate: transceiver.sender.track.getSettings().frameRate,
                  }
                : undefined,
              receiver: transceiver.receiver.track.id,
              state: transceiver.receiver.track.readyState,
              muted: transceiver.receiver.track.muted,
            })),
            stats,
          };
        }),
      ),
    };
  });
}

async function receivedAudioBytes(page: Page): Promise<{ peer: number; bytes: number }[]> {
  return page.evaluate(async () => {
    const debug = (window as Window & { __meshDebug?: { connections: RTCPeerConnection[] } })
      .__meshDebug;
    const counters: { peer: number; bytes: number }[] = [];
    for (const [peer, connection] of (debug?.connections ?? []).entries()) {
      if (connection.connectionState !== "connected") continue;
      let bytes = 0;
      (await connection.getStats()).forEach((report) => {
        if (report.type === "inbound-rtp" && report.kind === "audio") {
          bytes += report.bytesReceived ?? 0;
        }
      });
      counters.push({ peer, bytes });
    }
    return counters;
  });
}

async function receiving(page: Page, videos: number, audio: number): Promise<void> {
  const remoteVideos = page.locator("#remote-tracks video");
  await expect(remoteVideos).toHaveCount(videos, { timeout: 30_000 });
  await expect(page.locator("#remote-tracks audio")).toHaveCount(audio);
  if (videos > 0) {
    await expect
      .poll(
        () =>
          remoteVideos.evaluateAll((elements) =>
            elements.every((element) => {
              const video = element as HTMLVideoElement;
              return video.readyState >= 2 && video.videoWidth > 0;
            }),
          ),
        { timeout: 30_000 },
      )
      .toBe(true);
    const before = await remoteVideos.evaluateAll((elements) =>
      elements.map(
        (element) => (element as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames,
      ),
    );
    await expect
      .poll(
        () =>
          remoteVideos.evaluateAll(
            (elements, counts) =>
              elements.every(
                (element, index) =>
                  (element as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames >
                  counts[index],
              ),
            before,
          ),
        { timeout: 15_000 },
      )
      .toBe(true);
  }
  if (audio > 0) {
    const before = new Map(
      (await receivedAudioBytes(page)).map(({ peer, bytes }) => [peer, bytes]),
    );
    // Each live connection must receive new audio. Old accumulated RTP counters are
    // insufficient after unpublish/republish or a participant's leave/rejoin.
    await expect
      .poll(
        async () =>
          (await receivedAudioBytes(page)).filter(
            ({ peer, bytes }) => bytes > (before.get(peer) ?? 0),
          ).length,
      )
      .toBe(audio);
  }
}

async function installCanvasScreen(page: Page): Promise<void> {
  // This exercises a second live video publication, not operating-system screen capture.
  await page.evaluate(() => {
    navigator.mediaDevices.getDisplayMedia = async () => {
      const canvas = document.createElement("canvas");
      canvas.width = 640;
      canvas.height = 360;
      const context = canvas.getContext("2d") as CanvasRenderingContext2D;
      const stream = canvas.captureStream(10);
      const track = stream.getVideoTracks()[0];
      const draw = (time: number) => {
        if (track.readyState === "ended") return;
        context.fillStyle = `hsl(${Math.floor(time / 20) % 360} 70% 40%)`;
        context.fillRect(0, 0, canvas.width, canvas.height);
        context.fillStyle = "white";
        context.font = "28px sans-serif";
        context.fillText(`Second video source ${Math.round(time)}`, 30, 180);
        requestAnimationFrame(draw);
      };
      requestAnimationFrame(draw);
      return stream;
    };
  });
}

test("four independent peers send real WebRTC camera/audio and data, then release membership", async ({
  browser,
  page,
}, testInfo) => {
  const contexts = await Promise.all(
    Array.from({ length: 3 }, () =>
      browser.newContext({
        permissions: ["camera", "microphone"],
        viewport: { width: 1440, height: 1100 },
      }),
    ),
  );
  const pages = [page, ...(await Promise.all(contexts.map((context) => context.newPage())))];
  await Promise.all(pages.map(installMediaDiagnostics));
  const errors: string[] = [];
  for (const participant of pages)
    participant.on("pageerror", (error) => errors.push(error.message));
  try {
    await Promise.all(pages.map((participant) => participant.goto("http://localhost:1420")));
    await page.getByTestId("create-room").click();
    await joined(page);
    const invitation = await page.getByTestId("invitation-output").inputValue();
    expect(new URL(invitation).search).toBe("");
    for (const participant of pages.slice(1)) {
      await participant.getByTestId("invitation-input").fill(invitation);
      await participant.getByTestId("join-room").click();
      await joined(participant);
    }
    for (const participant of pages)
      await expect(participant.locator("#participant-count")).toHaveText("4 / 4 기기");
    await Promise.all(pages.map((participant) => participant.getByTestId("toggle-camera").click()));
    await Promise.all(pages.map((participant) => receiving(participant, 3, 3)));
    for (const [index, sender] of pages.entries()) {
      const message = `mesh-four-peer-proof-${index}`;
      await sender.getByTestId("message-input").fill(message);
      await sender.getByTestId("send-message").click();
      for (const receiver of pages.filter((participant) => participant !== sender)) {
        await expect(receiver.locator("#message-log")).toContainText(message);
      }
    }
    // Stop and republish on every peer, exercising receiver/transceiver reuse.
    await Promise.all(pages.map((participant) => participant.getByTestId("toggle-camera").click()));
    await Promise.all(pages.map((participant) => receiving(participant, 0, 0)));
    await Promise.all(pages.map((participant) => participant.getByTestId("toggle-camera").click()));
    await Promise.all(pages.map((participant) => receiving(participant, 3, 3)));
    await Promise.all(pages.map(installCanvasScreen));
    await Promise.all(pages.map((participant) => participant.getByTestId("toggle-screen").click()));
    await Promise.all(pages.map((participant) => receiving(participant, 6, 3)));
    // Secrets are functional in the UI, but deliberately redacted from exported evidence.
    await mkdir("proof", { recursive: true });
    await page.getByTestId("invitation-output").evaluate((element) => {
      (element as HTMLTextAreaElement).value = "[검증용 초대 링크 — 캡처에서 숨김]";
    });
    await page.screenshot({ path: "proof/four-peer-mesh.png", fullPage: true });
    await Promise.all(pages.map((participant) => participant.getByTestId("toggle-screen").click()));
    await Promise.all(pages.map((participant) => receiving(participant, 3, 3)));
    await pages[3].getByTestId("leave-room").click();
    for (const participant of pages.slice(0, 3)) {
      await expect(participant.locator("#participant-count")).toHaveText("3 / 4 기기");
      await expect(participant.locator("#remote-tracks video")).toHaveCount(2);
    }
    await pages[3].getByTestId("invitation-input").fill(invitation);
    await pages[3].getByTestId("join-room").click();
    await joined(pages[3]);
    await pages[3].getByTestId("toggle-camera").click();
    await Promise.all(pages.map((participant) => receiving(participant, 3, 3)));
    await pages[3].getByTestId("leave-room").click();
    for (const participant of pages.slice(0, 3)) {
      await expect(participant.locator("#participant-count")).toHaveText("3 / 4 기기");
      await expect(participant.locator("#remote-tracks video")).toHaveCount(2);
    }
    await page.getByTestId("toggle-camera").click();
    for (const participant of pages.slice(1, 3))
      await expect(participant.locator("#remote-tracks video")).toHaveCount(1);
    for (const participant of pages.slice(0, 3))
      await participant.getByTestId("leave-room").click();
    for (const participant of pages)
      await expect(participant.getByTestId("media-state")).toHaveAttribute("data-state", "idle");
    expect(errors).toEqual([]);
  } catch (error) {
    const diagnostics = await Promise.all(pages.map(mediaDiagnostics));
    const path = testInfo.outputPath("redacted-media-diagnostics.json");
    await writeFile(path, JSON.stringify(diagnostics, null, 2));
    await testInfo.attach("redacted-media-diagnostics", {
      path,
      contentType: "application/json",
    });
    throw error;
  } finally {
    await Promise.all(contexts.map((context) => context.close()));
  }
});

test("mobile-width laboratory remains usable without horizontal overflow", async ({ page }) => {
  const viewport = { width: 390, height: 844 };
  await page.setViewportSize(viewport);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Mesh laboratory." })).toBeVisible();
  await expect(page.getByTestId("create-room")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await mkdir("proof", { recursive: true });
  // Responsive proof records the tested viewport without requesting an off-viewport surface.
  const screenshot = await page.screenshot({
    path: "proof/mobile-laboratory.png",
    fullPage: false,
    scale: "css",
  });
  expect(screenshot.readUInt32BE(16)).toBe(viewport.width);
  expect(screenshot.readUInt32BE(20)).toBe(viewport.height);
});
