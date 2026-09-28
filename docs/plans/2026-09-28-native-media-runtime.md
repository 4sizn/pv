# Native media transport implementation plan

**Goal:** Extend the native mesh to transport camera, screen and microphone sources, with deterministic publication ownership and real native audio/video receive evidence.

**Architecture:** The Tokio runtime owns publication policy and manifests. The engine adapter owns fixed transceivers and native handles. Rust source adapters supply media; the TypeScript client projects metadata through readonly Rx streams. Product roles remain outside the SDK.

**Tech stack:** Pinned libwebrtc 0.3.50 / webrtc-sys 0.3.47, Rust 1.88, Tokio, Tauri, RxJS and the existing authenticated signaling service.

## Scope and decisions

Each participant may publish one camera, one screen and one microphone. The initial offer reserves two video slots and one audio slot; the answerer adopts these slots. Explicit kind/MID bindings accompany descriptions. Subsequent publish/unpublish uses sender track attachment and does not renegotiate SDP. Source IDs represent publication lifetimes; MIDs and receivers represent peer lifetimes.

This milestone implements native media transport with injected Rust sources and proves actual decoding using generated changing I420 frames and nonzero PCM. Physical device capture, permission prompts, screen selection and native presentation require their own adapters and hardware evidence. No frame bytes cross JSON IPC. The desktop's existing data probe remains a data probe.

The upstream binding needs narrowly vendored fixes: nullable sender track access, nullable SetTrack and the safe transceiver direction wrapper. Preserve upstream license/provenance and pin the local patch. Do not mutate the global Cargo cache or rebuild Chromium.

## Shared contracts

```rust
pub enum SourceKind { Camera, Screen, Microphone }
pub struct MediaSlot { pub kind: SourceKind, pub mid: String }
pub struct SourceDescriptor { pub id: String, pub kind: SourceKind }
pub struct RemoteSourceDescriptor {
    pub peer_id: String, pub id: String, pub kind: SourceKind, pub mid: String,
}
pub trait SourcePort: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn as_any(&self) -> &dyn std::any::Any;
    fn close(&mut self) -> PortFuture<'_, ()>;
}
// PeerPort additions:
// fn slots(&self) -> Vec<MediaSlot>;
// fn set_source(&mut self, kind: SourceKind, source: Option<&dyn SourcePort>)
//     -> Result<(), NativeError>;
// NativeMediaClient additions:
// async fn publish(&self, id: String, source: Box<dyn SourcePort>) -> Result<Snapshot, NativeError>;
// async fn unpublish(&self, id: String) -> Result<Snapshot, NativeError>;
```

`publish` transfers ownership on invocation, including rejection/cancellation cleanup. It requires joined state, a bounded nonempty ID and an unused source kind. Completion means local publication accepted; it does not acknowledge remote receipt. `unpublish` detaches every sender and closes the source; failed detach closes the affected peer so transmission cannot continue invisibly. Source Drop must cancel its tasks, and explicit close must await owned asynchronous work. Leave closes peers before sources; rejoin starts with no publications.

Descriptions carry `slots: [{kind, mid}]`. Signaling adds a bounded full manifest `{type:"sources", revision, sources:[{id,kind,mid}]}`. Runtime validates revision, ID/kind/MID uniqueness and agreement with negotiated slots. Old revisions cannot resurrect removed sources. Per-peer manifests are published after negotiation and after every local publication change. Browser and native media negotiation are distinct internal protocols; mixed sessions are not an interoperability claim.

Snapshots add `localSources` (maximum 3) and `remoteSources` (maximum 9). The renamed `NativeMediaClient` exposes these as readonly observables. IPC accepts no source handle or raw frame payload. Keep `NativeDataMessage` and `NativeDataSendResult` for data-specific contracts.

## Tasks and verification

- [x] Engine: vendor the pinned bindings and document exact modifications; implement fixed slot negotiation, safe attach/detach and native source/receiver boundaries in `crates/media-libwebrtc`. Run real native integration rather than relying on fake ports.
- [x] Runtime: implement publication ownership, full manifest validation and cleanup in `crates/media-runtime`. Extend injected-port tests for duplicate publications, rejection cleanup, publication before peer negotiation, late peers, detach failure, stale manifests, same-MID replacement and leave/rejoin.
- [x] SDK: rename the native client/facade lifecycle types; validate, freeze and project metadata in `packages/media-sdk/src/native`. Preserve all existing cancellation/teardown tests and extend malformed source snapshot tests. Update standalone and DOM-free consumers.
- [x] Composition: update `crates/media-native`, the Tauri host and existing native data tests to the renamed runtime; retain document ownership checks. No new main-thread capture work is introduced by this milestone.
- [x] Native proof: through the real signaling service, negotiate empty slots, publish video and audio both ways, observe changing decoded luma and nonzero audio energy, detach/re-publish on the same MID, retain data delivery, and finish source/receiver work on teardown.
- [x] Review layer boundaries and complete `bun run verify`, `bun run check:sdk-package`, `bun run verify:native` and `bun run verify:desktop`. Existing browser E2E remains a regression gate. New native integration is included automatically by the macOS CI command.
- [x] Record local evidence and limits in `docs/native-verification.md`; update architecture/protocol/crate docs.

Publication follows the reviewed implementation. The task completion message records the resulting commit, pushed branch and exact hosted CI run.

## Regression examples

```rust
client.publish("camera-1".into(), camera_source).await?;
assert_eq!(client.snapshot().local_sources.len(), 1);
client.unpublish("camera-1".into()).await?;
assert!(client.snapshot().local_sources.is_empty());
client.publish("camera-2".into(), replacement_source).await?;
client.leave().await?;
assert!(client.snapshot().local_sources.is_empty());
assert!(client.snapshot().remote_sources.is_empty());
```

```ts
// A native snapshot is the authority; the facade adds no media state machine.
client.localSources$.subscribe((sources) => renderSourceMetadata(sources));
client.remoteSources$.subscribe((sources) => renderRemoteMetadata(sources));
await client.destroy();
// All streams complete; late host responses cannot revive sources.
```

A standalone macOS lifecycle workload additionally observes thread/FD counts across repeated success, rejection, cancellation and partial initialization. A deliberately retained native sink must keep at least three runtime threads alive until released, proving the resource probe is sensitive to retained native ownership.

Generated media is deliberately identified as transport proof. It must never be reported as successful physical camera/microphone capture, native rendering, TURN traversal, or verification of unimplemented platforms.
