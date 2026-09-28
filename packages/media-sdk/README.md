# Media SDK

An independently buildable, role-neutral four-participant WebRTC mesh SDK. The initial runnable engine is the browser's WebRTC implementation. There is no native libwebrtc or Tauri dependency in this package.

```ts
import { createBrowserMediaClient, captureCamera, toBrowserTrack } from "@parentview/media-sdk/browser";

const client = createBrowserMediaClient({ signalingUrl: "ws://localhost:8787/ws", iceServers: [] });
client.tracks$.subscribe((tracks) => {
  // Attach new MediaStream([toBrowserTrack(remote.track)]) to a playback element.
});
await client.join({ roomId, roomToken, deviceToken });
const [track] = await captureCamera({ signal: captureAbort.signal });
if (track) {
  try { await client.publish({ id: "camera", kind: "camera", track }); }
  catch (error) { track.stop(); throw error; }
}
await client.leave(); // reusable, stops owned publications
await client.destroy(); // terminal, completes observables
```

Device registration and room invitation management belong to the application. Configure ICE servers explicitly; the SDK never chooses a public STUN service. Browser local-network validation does not prove TURN traversal or mobile/native functionality.

## Contracts and ownership

| Module | Owns / responsibility | Does not own | Allowed dependencies / teardown |
| --- | --- | --- | --- |
| `core/client.ts` | Public typed commands and readonly observables | Lifecycle decisions, native resources, product policy | Controller and core types; forwards teardown |
| `core/controller.ts` | Session generation, peer queues, offer collision policy, 20-second heartbeat subscription, local publication and remote track lifetime | Capture prompts, concrete engine or OS calls, product authorization | RxJS and ports only; leave invalidates tasks, closes peers/socket, stops owned tracks; destroy completes streams |
| `core/ports.ts`, `core/types.ts` | Opaque engine-neutral contracts | Implementations, DOM globals, mutable public subjects | Types and RxJS Observable only; resource close/stop contracts are explicit |
| `browser/peer.ts` | One peer connection, senders, data channels and listeners | Negotiation/retry policy | Browser APIs and core ports; close detaches all listeners/channels and closes connection |
| `browser/signaling.ts` | One socket, bounded handshake timeout, wire validation | Reconnect/retry and room policy | Browser APIs, RxJS and core ports; close rejects pending handshake and removes listeners/timer |
| `browser/tracks.ts` | Capture operation, native track conversion | Session or publication ownership | Browser media APIs and core ports; cancelled late results stop every acquired track |
| `browser/index.ts` | Concrete dependency wiring | Domain decisions | Client and browser adapters; lifetime returned through client |

`state$`, `peers$` and `tracks$` are snapshots. `messages$` and `errors$` are events. Errors never terminate a stream. `peers$` lists current remote membership, not proof that its data channel is open. `send()` surfaces not-yet-open/backpressure failures through `errors$`; applications can send after their connection/data readiness checks or retry a user action explicitly.

`publish()` transfers local-track ownership only on success; caller must stop failed/unpublished capture results. On success the controller stops the track during `unpublish`, `leave` or `destroy`. Remote handles are owned by the controller and must only be used for playback. Removing a remote publication removes its playback snapshot, while retaining the receiver handle until that peer closes: a new publication can reuse the same receiver/transceiver, and stopping it prematurely would permanently end its track. `CapturePort` is separate from session lifecycle; callers cancel their capture request when their workflow ends. Aborting a browser permission request cannot dismiss the platform prompt, but every late capture result is stopped before it is returned.

The controller serializes SDP and ICE mutations per peer, buffers early ICE, and resolves simultaneous offers with deterministic polite/impolite peer ordering. Only the lower participant ID creates the initial data channel. Any participant may publish and initiate later renegotiation. After applying a local description, the controller sends source metadata as `{ type: "track", id, kind, mid }`, binding the publication to its negotiated transceiver MID. Browser receiver track IDs can differ from sender track IDs, including when transceivers are reused, so native track IDs are never used to correlate remote publications. Departure/removal clears remote playback snapshots immediately.

The signaling connection is deliberately reusable after `leave`; no automatic reconnection is attempted. A signaling disconnect ends the current session. Pending join promises are cancelled promptly, and generation checks prevent an old negotiation continuation from sending or mutating a later session.

## Build and validation

From the workspace root, install with `bun install`; in this package run `bun run build` and `bun test tests`. The build first checks core against `ES2022` with no DOM or runtime ambient types, then emits ESM and declaration files under `dist`. `rxjs` is a peer dependency. Exports are the DOM-free core and separate `./browser` composition entry.

Tests cover late join/offer completion, repeated teardown, failed publication ownership, peer departure, early ICE, offer collisions, deferred renegotiation, socket reuse/wire validation and late capture cancellation. Actual browser mesh exchange is an integration acceptance check separate from these fake-adapter tests. Native media, remote input, TURN relay, network recovery and four-target OS validation remain separate work.
