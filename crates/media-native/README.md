# Native composition

`create_client()` constructs a raw libwebrtc adapter, bounded WebSocket adapter and the independent `pv-media-runtime` actor inside the current Tokio runtime. This crate wires concrete dependencies; it owns no product roles or consent rules.

The WebSocket adapter connects once to the existing authenticated `/ws` protocol, sends device/room credentials in the first frame, validates bounded server envelopes, routes ICE/SDP, and runs a heartbeat. It owns one socket task, a 64-entry write queue, 64 KiB frames, five-second writes and 45-second heartbeat deadline. Admission has a ten-second runtime deadline. Closing interrupts writes, attempts a bounded close handshake, and joins the task; a stalled task is explicitly aborted and joined before reporting cleanup failure. Drop aborts an outstanding task. Errors use safe fixed descriptions rather than echoing credentials, SDP or untrusted server text.

```rust,no_run
use pv_media_native::{create_client, JoinOptions};
# async fn example(options: JoinOptions) -> Result<(), pv_media_native::NativeError> {
let client = create_client()?;
client.join(options).await?;
// Wait for ready_peers before expecting data-channel acceptance.
let accepted = client.send("hello".into()).await?;
client.leave().await?;
client.destroy().await?;
# Ok(()) }
```

```sh
cargo test --locked -p pv-media-runtime
cargo test --locked -p pv-media-native -- --test-threads=1
cargo clippy --locked -p pv-media-runtime -p pv-media-libwebrtc -p pv-media-native --all-targets -- -D warnings
```

The integration test starts the real authenticated Tokio signaling server on a random loopback port and uses four actual native peers. It verifies every directed data path, Unicode payloads, engine readiness, peer departure/rejoin, rejected credentials, reusable leave and terminal destroy. It uses no public STUN/TURN server, browser, media capture or real microphone. Camera/audio/screen/rendering, TURN traversal and physical-device connectivity remain separate milestones.
