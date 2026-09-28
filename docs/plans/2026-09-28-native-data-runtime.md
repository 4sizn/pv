# Native data runtime implementation plan

**Goal:** Establish an independently buildable, role-neutral Rust/Tokio native WebRTC data session, with authenticated mesh signaling and a thin Rx/Tauri interface on macOS.

**Architecture:** Rust is the only session/membership/negotiation authority. A runtime crate depends on engine and signaling ports; concrete WebRTC and WebSocket adapters live outside it. The TypeScript native entry projects Rust snapshots and events and never constructs the browser `MediaController`. ParentView roles and authorization remain in the service domain.

**Tech stack:** Existing Rust 1.88, Tokio, Tauri and RxJS; raw `libwebrtc = "=0.3.50"` bindings, without the `livekit` room/client SDK. Google/libwebrtc is an engine dependency; mesh, signaling, lifecycle and the public SDK are ours.

## Scope and acceptance

The approved next milestone is native media. This first executable slice covers native data channels, the prerequisite transport and its teardown. Camera/microphone/screen capture and native rendering stay unsupported. The browser laboratory remains its existing browser-media validation surface. Native capability reporting adds `nativeDataChannels` without claiming `nativeMedia` or remote input.

Completion requires actual native peers joining the existing authenticated signaling server, exchanging ordered UTF-8 strings through DTLS/SCTP, leaving and rejoining; bounded queues, cancellation and deterministic teardown; tests of the injected Rx facade and host ownership. No raw media frames pass through JSON IPC. No public SDP/ICE workflow is added to the service API.

## Shared contract

The native Rust command API is mirrored by this DOM-free SDK contract. Wire data uses camelCase. Errors contain a stable code and safe message, never credentials or SDP.

```ts
type NativeDataState = "idle" | "joining" | "joined" | "leaving" | "destroyed";
interface NativeDataSnapshot {
  readonly revision: number;
  readonly state: NativeDataState;
  readonly peerId: string | null;
  readonly peers: readonly string[];
  readonly readyPeers: readonly string[];
}
interface NativeDataError { readonly code: string; readonly message: string }
type NativeDataEvent =
  | { readonly type: "message"; readonly peerId: string; readonly data: string }
  | { readonly type: "error"; readonly error: NativeDataError };
interface NativeDataBatch {
  readonly snapshot: NativeDataSnapshot;
  readonly events: readonly NativeDataEvent[];
}
interface NativeDataJoinOptions {
  readonly signalingUrl: string;
  readonly roomId: string;
  readonly roomToken: string;
  readonly deviceToken: string;
  readonly iceServers?: readonly {
    readonly urls: readonly string[];
    readonly username?: string;
    readonly credential?: string;
  }[];
}
interface NativeDataSendResult {
  readonly acceptedPeerIds: readonly string[];
  readonly failures: readonly { readonly peerId: string; readonly message: string }[];
}
interface NativeDataTransportPort {
  open(): Promise<NativeDataSnapshot>;
  readBatch(): Promise<NativeDataBatch>;
  join(options: NativeDataJoinOptions): Promise<NativeDataSnapshot>;
  send(data: string): Promise<NativeDataSendResult>;
  leave(): Promise<NativeDataSnapshot>;
  destroy(): Promise<NativeDataSnapshot>;
}
```

Rust exports corresponding `Snapshot`, `Event`, `EventBatch`, `JoinOptions`, `SendResult`, `NativeError` and cloneable `NativeDataClient` handle with async methods `read_batch`, `join`, `send`, `leave`, `destroy`, plus a synchronous `snapshot`. The composition entry `pv_media_native::create_client()` returns `Result<NativeDataClient, NativeError>` within a Tokio runtime. `destroy` is terminal and idempotent; `leave` is reusable. Native commands return authoritative snapshots so a completed command can be reflected immediately without a second TS state machine. `send` reports local engine acceptance per peer, not remote application delivery.

## Tasks

- [x] Repair the first hosted Linux E2E capture failure, preserving all layout/media assertions and zero retries. Verify the resulting hosted run.
- [x] `crates/media-runtime/{Cargo.toml,src/lib.rs,src/types.rs,src/ports.rs,src/runtime.rs,tests/}`: implement generic/injected engine/signaling ports and the native session owner. Bound commands/events/messages, reject oversized UTF-8, serialize SDP/ICE per peer, buffer early ICE, make joining interruptible, invalidate late callbacks, close failed peers independently and finish all owned tasks on teardown. Runtime must not depend on Tauri, libwebrtc or product services.
- [x] `crates/media-libwebrtc/{Cargo.toml,src/lib.rs}`: isolate the raw engine. Copy callback payloads into a bounded event queue; callbacks must never close/reconfigure native objects under their handler mutex. Unregister callbacks, close channels/connections and release handles before dropping the factory. Native callbacks and sender buffering both have explicit limits.
- [x] `crates/media-native/{Cargo.toml,src/lib.rs,src/signaling.rs,tests/native_session.rs}`: compose the runtime and concrete adapters. Connect to existing `/ws`, authenticate once, enforce join/heartbeat/write deadlines, route all ICE, validate wire messages and close pending sockets on cancellation. Only the lexicographically smaller authenticated peer ID initiates the data channel/offer; data-only sessions do not need speculative media renegotiation. Integration tests use the real signaling service and real native engine, explicit deadlines and isolated credentials.
- [x] `packages/media-sdk/src/native/{types.ts,client.ts,index.ts}` and native facade tests: implement `NativeDataClient.create({ transport })`, readonly `state$`, `peers$`, `messages$`, `errors$`, command methods and one long-poll event pump. Apply only Rust snapshots, reject stale revisions and ignore late completions after destruction. Never expose Subjects or import Tauri/DOM. Use ES private fields and existing repository formatting.
- [x] `apps/desktop/src/native-transport.ts` and `apps/desktop/src-tauri/src/native_data.rs`: implement typed command transport and window-owned client registry. Keep at most one pending event read per client; return bounded batches rather than accumulating unacknowledged IPC events. Check command ownership by actual window label. Window/application teardown cancels native clients. Native availability is platform-gated; browser media remains unchanged.
- [x] `apps/desktop/src/native-data-probe.ts`, its contract tests and minimal laboratory wiring: provide a Tauri-only native data check. Two independently authenticated native clients share one ephemeral test room, wait for actual Rust-reported `readyPeers`, exchange messages both ways and destroy resources before reporting success. This exercises the real Rx facade and IPC, not a test-only engine shortcut.
- [x] Package exports, DOM-free build, independent consumer and architecture checks cover the native facade. Rust crate dependency checks prohibit runtime → concrete adapters/Tauri/product services and adapter → application dependencies. Native tests cover cancellation, teardown, malformed input, queue overflow, peer departure and real transport.
- [x] Add documented native verification and macOS CI without forcing Ubuntu jobs to compile libwebrtc with incompatible system tooling. Preserve binary license notices; pin crate graph in Cargo.lock and record native artifact identity/integrity. Review the final diff and run the appropriate complete gates before committing.

## Verification examples

```rust
let before = client.snapshot();
assert_eq!(before.state, State::Idle);
client.join(credentials).await?;
assert_eq!(client.snapshot().state, State::Joined);
client.leave().await?;
client.leave().await?;
assert_eq!(client.snapshot().state, State::Idle);
client.destroy().await?;
assert_eq!(client.snapshot().state, State::Destroyed);
```

```ts
const client = await NativeDataClient.create({ transport });
// A pending transport command does not invent a native lifecycle transition.
const join = client.join(options);
expect(observedStates).toEqual(["idle"]);
transport.resolveJoin({ revision: 1, state: "joined", peerId: "a", peers: [] });
await join;
expect(observedStates).toEqual(["idle", "joined"]);
await client.destroy();
expect(streamsCompleted).toBe(true);
```

## Build feasibility and limits

The initial target is macOS. Upstream raw bindings provide macOS ARM64/X64 prebuilt engines. The researched ARM64 release is `webrtc-89d790b`, archive `webrtc-mac-arm64-release.zip` (277,148,750 bytes), SHA-256 `9f25fea48588deac18d68e120d18b33af7ef10f921f74b7c57f66b642262c348`. Actual Rust 1.88 compilation is a gate, not an assumption. Available disk is limited; use the existing Cargo target/cache and no full Chromium source checkout. Linux native builds require newer clang/libc++ tooling than the existing runner and remain separately gated. TURN, physical capture, cross-device reachability and mobile/Windows native operation remain unverified.

Sources: [raw binding manifest](https://github.com/livekit/rust-sdks/blob/libwebrtc/v0.3.50/libwebrtc/Cargo.toml), [engine artifacts](https://github.com/livekit/rust-sdks/releases/tag/webrtc-89d790b), [build helper](https://github.com/livekit/rust-sdks/blob/libwebrtc/v0.3.50/webrtc-sys/build/src/lib.rs), [native data-channel callbacks](https://github.com/livekit/rust-sdks/blob/libwebrtc/v0.3.50/libwebrtc/src/native/data_channel.rs).

Completed local evidence: [native verification report](../native-verification.md). The first hosted baseline run was repaired and passed; hosted verification of the native branch follows publication.
