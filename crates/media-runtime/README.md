# Native media runtime

Owns one authoritative role-neutral session actor: admission state, up to three remote peers, deterministic offer direction, ordered SDP/ICE changes, publication ownership, source manifests, readiness, bounded messages and teardown. Depends only on Tokio, Serde and its injected engine/signaling contracts. It does not import Tauri, concrete WebRTC, WebSocket, capture APIs or product policy.

`NativeMediaClient` is a cloneable command handle. `snapshot()` is synchronous; `join`, `send`, `publish`, `unpublish`, `media_stats`, `leave`, `destroy`, and `read_batch` are asynchronous. JSON DTOs use camelCase. `peers` records current peer connections; `readyPeers` records engine data-channel readiness. Send acceptance is local engine acceptance, not a delivery acknowledgement.

Each session owns at most one camera, screen and microphone source. `publish(id, Box<dyn SourcePort>)` consumes ownership on invocation, including rejection cleanup. It requires a joined session and a distinct bounded ID/kind. Completion means local ownership is established; negotiating or later peers receive the source once their slots exist. Unpublish detaches senders before closing the source. A failed detach closes the affected peer, preventing hidden continued transmission. Sources must cancel producers in Drop and join them in explicit close.

The engine owns three fixed slots, exposed through injected `PeerPort` operations. Runtime sends full revisioned source manifests and validates remote IDs, kinds and MIDs against those slots. Source metadata is bounded to three local and nine remote descriptors. Sources and remote descriptors clear on leave. The diagnostic `media_stats` command returns scalar native receive observations; it carries no raw frames and is not a Tauri command. Sampled content signatures do not claim an exact publication frame boundary.

One actor owns its peers and signaling port. Commands are bounded at 32, callback input at 128, unread public events at 64, batches at 32 and UTF-8 data at 16,384 bytes. Early ICE is bounded at 64 candidates per peer; sender buffering is an adapter responsibility. Overflow closes the session explicitly. Leave/destroy use a separate control watch slot so saturation cannot prevent cancellation. Native callbacks only enqueue owned events.

Each session and each peer incarnation has a generation; old callbacks cannot mutate a replacement peer after departure/rejoin. An event read is exclusive across handle clones and returns `read-in-progress` rather than queueing another consumer. Dropping a pending read is safe. Leave closes all peers and signaling and clears unread data from the previous session; it permits joining again. Destroy also releases the engine factory, becomes terminal, and completes idempotently. Cleanup failures/timeouts are returned and emitted even though resources move to Idle/Destroyed; repeated destruction retains a terminal cleanup failure. Dropping the last handle requests destruction; applications should await `destroy()` before shutting down Tokio.

```sh
cargo test --locked -p pv-media-runtime
```

Tests inject ports for cancellation during admission and SDP, saturated command/callback/consumer queues, readiness, failed-peer isolation, early ICE, stale callbacks, terminal commands, UTF-8 byte bounds, publication/manifest ownership and final-handle teardown. No browser/native engine is required for this crate.
