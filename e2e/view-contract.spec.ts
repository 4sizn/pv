import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { ModuleKind, ScriptTarget, transpileModule } from "typescript";
import type { LabView } from "../apps/desktop/src/lab-view";

test("renderer rebinds a replaced playback handle without replacing the media element", async ({
  page,
}) => {
  const source = await readFile(
    new URL("../apps/desktop/src/lab-view.ts", import.meta.url),
    "utf8",
  );
  const { outputText } = transpileModule(source, {
    compilerOptions: { target: ScriptTarget.ES2022, module: ModuleKind.ESNext },
  });
  await page.goto("about:blank");
  const result = await page.evaluate(async (moduleSource) => {
    const moduleUrl = URL.createObjectURL(new Blob([moduleSource], { type: "text/javascript" }));
    const tracks: MediaStreamTrack[] = [];
    let view: LabView | undefined;
    const root = document.createElement("div");
    document.body.append(root);
    try {
      const { LabView: View } = (await import(moduleUrl)) as { LabView: typeof LabView };
      for (const color of ["blue", "green"]) {
        const canvas = document.createElement("canvas");
        canvas.width = 16;
        canvas.height = 16;
        const context = canvas.getContext("2d");
        if (!context) throw new Error("Canvas 2D context unavailable");
        context.fillStyle = color;
        context.fillRect(0, 0, canvas.width, canvas.height);
        const track = canvas.captureStream(0).getVideoTracks()[0];
        if (!track) throw new Error("Canvas capture did not produce a video track");
        tracks.push(track);
      }
      const [trackA, trackB] = tracks;
      let stops = 0;
      const sourceFor = (track: MediaStreamTrack) => ({
        peerId: "same-peer",
        id: "same-camera-source",
        kind: "camera" as const,
        track: {
          id: track.id,
          kind: "video" as const,
          stop: () => {
            stops += 1;
            track.stop();
          },
        },
      });
      const sourceA = sourceFor(trackA);
      const sourceB = sourceFor(trackB);
      view = new View(root, (port) => {
        const track = tracks.find((candidate) => candidate.id === port.id);
        if (!track) throw new Error("Unknown playback handle");
        return track;
      });
      view.setTracks([sourceA]);
      const card = root.querySelector("#remote-tracks article");
      const media = root.querySelector("#remote-tracks video");
      if (!(media instanceof HTMLVideoElement)) throw new Error("Remote video was not rendered");
      const initiallyA = (media.srcObject as MediaStream).getVideoTracks()[0] === trackA;

      let rejectPlayback!: (reason: Error) => void;
      media.play = () =>
        new Promise<void>((_resolve, reject) => {
          rejectPlayback = reject;
        });
      view.setTracks([sourceB]);
      const reboundStream = media.srcObject;
      const reboundToB = (reboundStream as MediaStream).getVideoTracks()[0] === trackB;
      const keptMediaElement = root.querySelector("#remote-tracks video") === media;
      const keptSingleCard =
        root.querySelectorAll("#remote-tracks article").length === 1 &&
        root.querySelector("#remote-tracks article") === card;
      view.setTracks([sourceB]);
      const unchangedHandleKeepsStream = media.srcObject === reboundStream;

      view.setTracks([]);
      const removedCard = root.querySelectorAll("#remote-tracks article").length === 0;
      const clearedPlayback = media.srcObject === null;
      view.destroy();
      view = undefined;
      rejectPlayback?.(new Error("Late autoplay rejection"));
      await Promise.resolve();
      return {
        initiallyA,
        reboundToB,
        keptMediaElement,
        keptSingleCard,
        unchangedHandleKeepsStream,
        removedCard,
        clearedPlayback,
        callerTracksRemainLive: tracks.every((track) => track.readyState === "live"),
        callerStopCalls: stops,
        noNoticeAfterDestroy: root.querySelector("#notice")?.textContent === "",
      };
    } finally {
      view?.destroy();
      for (const track of tracks) track.stop();
      root.remove();
      URL.revokeObjectURL(moduleUrl);
    }
  }, outputText);

  expect(result).toEqual({
    initiallyA: true,
    reboundToB: true,
    keptMediaElement: true,
    keptSingleCard: true,
    unchangedHandleKeepsStream: true,
    removedCard: true,
    clearedPlayback: true,
    callerTracksRemainLive: true,
    callerStopCalls: 0,
    noNoticeAfterDestroy: true,
  });
});
