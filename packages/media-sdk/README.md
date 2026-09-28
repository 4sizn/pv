# Media SDK

An independently buildable, role-neutral four-participant WebRTC mesh SDK. The browser entry uses the browser's WebRTC implementation. The separate native entry projects a host-provided Rust data runtime through an injected transport; this TypeScript package imports neither libwebrtc nor Tauri.

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
| `browser/peer.ts` | One peer connection, publication transceivers, data channels and listeners | Negotiation/retry policy, capture ownership | Browser APIs and core ports; removal stops the publication transceiver; close detaches all listeners/channels and closes connection |
| `browser/signaling.ts` | One socket, bounded handshake timeout, wire validation | Reconnect/retry and room policy | Browser APIs, RxJS and core ports; close rejects pending handshake and removes listeners/timer |
| `browser/tracks.ts` | Capture operation, native track conversion | Session or publication ownership | Browser media APIs and core ports; cancelled late results stop every acquired track |
| `browser/index.ts` | Concrete dependency wiring | Domain decisions | Client and browser adapters; lifetime returned through client |
| `native/client.ts` | Public native commands, readonly snapshots/events and one bounded IPC read pump | Session transitions, SDP/ICE, browser controller, product policy | RxJS and native transport port only; destroy cancels pending facade waits, awaits host destruction and completes streams |
| `native/types.ts`, `native/index.ts` | DOM-free native data contract and public entry | Tauri, OS or engine construction | Types, RxJS Observable and native facade; concrete transport belongs to the host |

`state$`, `peers$` and `tracks$` are snapshots. `messages$` and `errors$` are events. Errors never terminate a stream. `peers$` lists current remote membership, not proof that its data channel is open. `send()` surfaces not-yet-open/backpressure failures through `errors$`; applications can send after their connection/data readiness checks or retry a user action explicitly.

`publish()` transfers local-track ownership only on success; caller must stop failed/unpublished capture results. On success the controller stops the track during `unpublish`, `leave` or `destroy`. Remote handles are owned by the controller and must only be used for playback. Removing a remote publication removes its playback snapshot, while retaining the receiver handle until that peer closes: a new publication can reuse the same receiver/transceiver, and stopping it prematurely would permanently end its track. `CapturePort` is separate from session lifecycle; callers cancel their capture request when their workflow ends. Aborting a browser permission request cannot dismiss the platform prompt, but every late capture result is stopped before it is returned.

The controller serializes SDP and ICE mutations per peer, buffers early ICE, and resolves simultaneous offers with deterministic polite/impolite peer ordering. Only the lower participant ID creates the initial data channel. Any participant may publish and initiate later renegotiation. After applying a local description, the controller sends source metadata as `{ type: "track", id, kind, mid }`, binding the publication to its negotiated transceiver MID. Browser receiver track IDs can differ from sender track IDs, including when transceivers are reused, so native track IDs are never used to correlate remote publications. Departure/removal clears remote playback snapshots immediately.

The browser adapter gives each local publication a dedicated `sendonly` transceiver. This avoids a reproduced Chromium 153 failure: attaching a camera to a remotely created transceiver while answering an offer, then rolling back a later colliding offer, can detach that camera's engine source while its track still reports live. The deterministic `e2e/browser-peer.spec.ts` regression uses the actual adapter and requires both existing camera senders and second video sources to keep encoding. Unpublish stops that publication's transceiver, allowing the browser to recycle the negotiated m-line; capture-track ownership remains with the controller. The browser regression also checks that repeated completed publish/remove negotiations keep a stable m-line count. There is no transceiver pool or automatic engine retry.

The signaling connection is deliberately reusable after `leave`; no automatic reconnection is attempted. A signaling disconnect ends the current session. Pending join promises are cancelled promptly, and generation checks prevent an old negotiation continuation from sending or mutating a later session.

## Native data entry

`@parentview/media-sdk/native` exports `NativeDataClient` and `NativeDataTransportPort`. This is a data-only client: it has no capture, publication or playback API. The browser `MediaController` owns browser sessions; the Rust actor owns native session, membership, readiness and negotiation state. The native facade only validates and projects Rust snapshots and events. These are separate implementations, never competing state owners for one connection.

```ts
import { NativeDataClient, type NativeDataTransportPort } from "@parentview/media-sdk/native";
import { filter, firstValueFrom, timeout } from "rxjs";

// Supplied by the host. The reusable SDK does not import a Tauri/OS adapter.
declare const transport: NativeDataTransportPort;
const client = await NativeDataClient.create({ transport });
const incoming = client.messages$.subscribe(({ peerId, data }) => {
  // Handle application messages; remote delivery requires your own receipt/correlation check.
});
try {
  await client.join({ signalingUrl, roomId, roomToken, deviceToken, iceServers });
  await firstValueFrom(client.readyPeers$.pipe(
    filter((ids) => ids.includes(expectedPeerId)),
    timeout({ first: 15_000 }),
  ));
  const result = await client.send("native data check");
  if (result.failures.length || !result.acceptedPeerIds.includes(expectedPeerId)) {
    throw new Error("The native engine did not accept this send for the expected peer");
  }
  await client.leave(); // reusable; Rust cancels pending session work
} finally {
  incoming.unsubscribe();
  await client.destroy(); // terminal and idempotent; await host cleanup
}
```

Native `state$`, `peers$` and `readyPeers$` are readonly snapshots. `peers$` describes native peer membership; `readyPeers$` contains peers whose actual native data channel is open. A completed `join()` or nonempty membership snapshot does not guarantee channel readiness. `messages$` and `errors$` are events, and recoverable errors are values rather than terminal stream errors. The transport permits one pending read per client and returns at most 32 events per batch.

Native `send()` accepts a UTF-8 string of at most 16,384 bytes and returns `acceptedPeerIds` plus per-peer `failures`. Acceptance means the local engine accepted the send; it does not acknowledge delivery to the remote application. Subscribe before sending when a workflow must prove a specific receive event. The native client does not retry sends or reconnect automatically, and readiness can change between checking a snapshot and calling `send()`.

Unsubscribing an Rx observer or timing out a wait does not cancel a native operation. Use `leave()` to cancel the current native session while retaining a reusable client, and await `destroy()` for terminal resource cleanup. Destruction rejects pending facade commands, ignores their late completions, stops event reads and completes the observables. A host cleanup failure rejects destruction; the facade remains terminal and later destruction calls share the same result. Failed creation also requests transport cleanup. Host document/window teardown supplies a final ownership boundary independently of page JavaScript.

The desktop composition provides `createTauriNativeDataClient()` and a developer probe that creates two native peers and checks exact bidirectional delivery plus cleanup. Native data capability is currently gated to macOS. `nativeMedia` and remote input remain false; Android, iOS and Windows native data operation, native capture/rendering and TURN traversal remain unverified. Actual macOS Tauri IPC/Rx delivery was verified separately from facade tests; see [the native report](../../docs/native-verification.md).

## Build and validation

From the workspace root, install with `bun install`; in this package run `bun run build` and `bun test tests`. The build first checks core and native facade against `ES2022` with no DOM or runtime ambient types, then emits ESM and declaration files under `dist`. `rxjs` is a peer dependency. Exports are the DOM-free core, `./native` facade and separate `./browser` composition entry.

Tests cover late join/offer completion, repeated teardown, failed publication ownership, peer departure, early ICE, offer collisions, deferred renegotiation, socket reuse/wire validation and late capture cancellation. Native facade tests cover snapshot authority, actual readiness projection, event/command bounds, stale and late completion, cancellation and failed cleanup. Actual browser mesh and native engine exchange are separate integration acceptance checks; facade or fake-adapter tests cannot replace them. Native media, remote input, TURN relay, network recovery and four-target OS validation remain separate work.
