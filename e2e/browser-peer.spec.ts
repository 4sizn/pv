import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type * as BrowserSdk from "../packages/media-sdk/src/browser/index.js";
import type { PeerPort } from "../packages/media-sdk/src/index.js";

test("browser adapter keeps an answered camera sending after later offer rollback", async ({
  page,
}) => {
  const moduleSource = execFileSync(
    "bun",
    [
      "build",
      fileURLToPath(new URL("../packages/media-sdk/src/browser/index.ts", import.meta.url)),
      "--target",
      "browser",
    ],
    { encoding: "utf8" },
  );
  await page.goto("/");
  const result = await page.evaluate(async (source) => {
    const moduleUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
    const connections: RTCPeerConnection[] = [];
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class extends NativePeer {
      constructor(configuration?: RTCConfiguration) {
        super(configuration);
        connections.push(this);
      }
    };
    const captures: MediaStreamTrack[] = [];
    const peers: PeerPort[] = [];
    const iceFailures: string[] = [];
    try {
      const { BrowserPeerFactory, fromBrowserTrack } = (await import(
        moduleUrl
      )) as typeof BrowserSdk;
      const factory = new BrowserPeerFactory();
      const a = factory.create("a");
      const b = factory.create("b");
      peers.push(a, b);
      for (const [sender, receiver] of [
        [a, b],
        [b, a],
      ]) {
        sender.events$.subscribe((event) => {
          if (event.type === "ice")
            void receiver
              .addIceCandidate(event.candidate)
              .catch((error: Error) => iceFailures.push(error.name));
        });
      }
      const negotiate = async (offerer: PeerPort, answerer: PeerPort) => {
        const offer = await offerer.createOffer();
        await offerer.setLocalDescription(offer);
        await answerer.setRemoteDescription(offer);
        const answer = await answerer.createAnswer();
        await answerer.setLocalDescription(answer);
        await offerer.setRemoteDescription(answer);
      };
      const until = async (predicate: () => Promise<boolean>) => {
        const deadline = performance.now() + 5_000;
        do {
          if (await predicate()) return;
          await new Promise((resolve) => setTimeout(resolve, 25));
        } while (performance.now() < deadline);
        throw new Error("Browser media did not progress after negotiation");
      };
      const addCamera = async (peer: PeerPort, id: string) => {
        const stream = await navigator.mediaDevices.getUserMedia({ video: true });
        const track = stream.getVideoTracks()[0];
        captures.push(track);
        peer.addTrack(id, fromBrowserTrack(track));
        return track;
      };
      const frames = async (peer: PeerPort, id: string) => {
        const mid = peer.trackBindings().find((binding) => binding.id === id)?.mid;
        let count = 0;
        (await connections[peers.indexOf(peer)].getStats()).forEach((report) => {
          if (report.type === "outbound-rtp" && report.mid === mid)
            count += report.framesEncoded ?? 0;
        });
        return count;
      };
      a.createDataChannel();
      await negotiate(a, b);
      await until(async () =>
        connections.every((connection) => connection.connectionState === "connected"),
      );

      const oldA = await addCamera(a, "old");
      const oldB = await addCamera(b, "old");
      await negotiate(a, b);
      if (!b.trackBindings().some((binding) => binding.id === "old")) await negotiate(b, a);
      await until(async () => (await frames(a, "old")) > 0 && (await frames(b, "old")) > 0);
      a.removeTrack("old");
      b.removeTrack("old");
      const removalPreservesCapture = oldA.readyState === "live" && oldB.readyState === "live";
      oldA.stop();
      oldB.stop();
      await negotiate(a, b);

      await addCamera(a, "camera");
      const offer = await a.createOffer();
      await a.setLocalDescription(offer);
      await b.setRemoteDescription(offer);
      // Deterministic reproduction of capture completing while a remote offer is pending.
      await addCamera(b, "camera");
      const answer = await b.createAnswer();
      await b.setLocalDescription(answer);
      await a.setRemoteDescription(answer);
      // A dedicated sender needs its own m-line; addTrack's reused receiver already has one.
      if (!b.trackBindings().some((binding) => binding.id === "camera")) await negotiate(b, a);
      await until(async () => (await frames(a, "camera")) > 2 && (await frames(b, "camera")) > 2);
      const before = await Promise.all(peers.map((peer) => frames(peer, "camera")));

      await addCamera(a, "second");
      await addCamera(b, "second");
      const [offerA, offerB] = await Promise.all(peers.map((peer) => peer.createOffer()));
      await a.setLocalDescription(offerA);
      await b.setLocalDescription(offerB);
      await b.setLocalDescription({ type: "rollback" });
      await b.setRemoteDescription(offerA);
      const collisionAnswer = await b.createAnswer();
      await b.setLocalDescription(collisionAnswer);
      await a.setRemoteDescription(collisionAnswer);
      await negotiate(b, a);
      // Both old camera senders and both new video sources must still produce fresh frames.
      await until(async () => {
        const camera = await Promise.all(peers.map((peer) => frames(peer, "camera")));
        const second = await Promise.all(peers.map((peer) => frames(peer, "second")));
        return (
          camera.every((count, index) => count > before[index] + 2) &&
          second.every((count) => count > 2)
        );
      });
      // Removing an owned RTP resource must release its negotiated slot without taking
      // ownership of the caller's capture track. Repeated toggles must not grow SDP forever.
      const reusable = await addCamera(a, "cycle");
      await negotiate(a, b);
      const mediaSections: number[] = [];
      for (let index = 0; index < 8; index++) {
        mediaSections.push(connections[0].localDescription?.sdp.split("\nm=").length ?? 0);
        a.removeTrack("cycle");
        await negotiate(a, b);
        a.addTrack("cycle", fromBrowserTrack(reusable));
        await negotiate(a, b);
      }
      return {
        cameraProgress: true,
        secondSourceProgress: true,
        removalPreservesCapture,
        repeatedRemovalPreservesCapture: reusable.readyState === "live",
        mediaSections,
        iceFailures,
      };
    } finally {
      for (const peer of peers) peer.close();
      for (const track of captures) track.stop();
      window.RTCPeerConnection = NativePeer;
      URL.revokeObjectURL(moduleUrl);
    }
  }, moduleSource);
  expect(result).toMatchObject({
    cameraProgress: true,
    secondSourceProgress: true,
    removalPreservesCapture: true,
    repeatedRemovalPreservesCapture: true,
    iceFailures: [],
  });
  expect(new Set(result.mediaSections).size).toBe(1);
});
