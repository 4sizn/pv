# Native composition

`create_client()` constructs a raw libwebrtc adapter, bounded WebSocket adapter and the independent `pv-media-runtime` actor inside the current Tokio runtime. This crate wires concrete dependencies; it owns no product roles or consent rules.

`create_client_with_engine(Arc<dyn EngineFactory>)` shares an engine factory with native source adapters. The resulting `NativeMediaClient` accepts opaque owned `SourcePort` values. The composition root supplies authenticated signaling; capture scheduling remains with the source owner.

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

Native source ingress is provided separately by `pv-media-libwebrtc`. A capture adapter creates initialized I420 video frames or 10 ms mono PCM frames at 48 kHz, supplies them through `VideoInput`/`AudioInput`, and owns its producer tasks. It wraps the source so `close()` cancels and joins those tasks and delegates native source cleanup. Do not run a detached producer or pass decoded frames through Tauri JSON.

```rust,no_run
use std::sync::Arc;
use pv_media_libwebrtc::LibWebRtcFactory;
use pv_media_native::{create_client_with_engine, JoinOptions, NativeError, SourceKind};
# async fn example(options: JoinOptions) -> Result<(), NativeError> {
let engine = Arc::new(LibWebRtcFactory::default());
let client = create_client_with_engine(engine.clone())?;
client.join(options).await?;
let (source, input) = engine.video_source(SourceKind::Camera, 320, 180)?;
client.publish("camera-1".into(), Box::new(source)).await?;
// An actual producer must feed owned initialized frames through input.capture(frame).
// This example reserves ownership only; it does not acquire a camera or emit frames.
client.unpublish("camera-1".into()).await?;
// All clones of input now reject new frames because the source has been closed.
drop(input);
client.destroy().await?;
# Ok(()) }
```

```sh
cargo test --locked -p pv-media-runtime
cargo test --locked -p pv-media-native -- --test-threads=1
cargo clippy --locked -p pv-media-runtime -p pv-media-libwebrtc -p pv-media-native --all-targets -- -D warnings
```

The data integration test starts the real authenticated Tokio signaling server on a random loopback port and uses four actual native peers. It verifies every directed data path, Unicode payloads, engine readiness, peer departure/rejoin, rejected credentials, reusable leave and terminal destroy. The media integration separately exercises generated video and PCM with the real native encoder/decoder, fixed slots, source removal/republication and teardown. These tests use no public STUN/TURN server, browser, physical media capture or real microphone. Physical capture/rendering, TURN traversal and cross-device connectivity remain separate milestones.
