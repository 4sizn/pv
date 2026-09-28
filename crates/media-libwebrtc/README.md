# Native engine adapter

Owns libwebrtc factory, peer, data-channel and fixed media-slot handles, native source ingress, bounded decoded sinks, and callback conversion. Implements `pv-media-runtime` ports; does not own rooms, product roles, retry policy, Tauri IPC, or OS capture. Native WebRTC owns its network, worker and signaling threads; the Rust session actor owns when handles are created, configured and released.

The engine dependency is the raw `libwebrtc = "=0.3.50"` binding. The `livekit` room/client SDK is not a dependency. Peer callbacks enqueue bounded owned data, never await or close/reconfigure a peer while native callback mutexes are held. The adapter clears observers, synchronously detaches decoded sinks and sender tracks, then closes channels and connections. Drop is also idempotent. A data send requires an open channel, <=16 KiB UTF-8 payload, and <=1 MiB queued sender bytes.


## Fixed media slots and frame ownership

Each peer connection reserves one camera video, one screen video and one microphone audio slot. Only the initial offerer creates transceivers. The answerer adopts the offered MIDs and sets SendRecv before answering. Both sides bind and detach tracks on those slots without further SDP; source identifiers are runtime-owned metadata, separate from the stable receiver track/MID. The initial profile uses software VP8 video and the engine's negotiated audio codec.

`LibWebRtcFactory::video_source(kind, width, height)` returns an owned `LibWebRtcSource` and cloneable `VideoInput`; dimensions are even and bounded to 1920×1080. `VideoInput::capture` consumes an initialized I420 frame so callers cannot mutate a buffer retained by the engine. `audio_source()` returns the same owner plus `AudioInput::capture_10ms`, accepting exactly 480 signed 16-bit mono PCM samples at 48 kHz. It uses the pinned binding's synchronous zero-queue fast path. These ingress adapters do not acquire devices or create producer tasks. Closing or dropping the source serializes against pushes, releases ingress/track handles and rejects later pushes. Producer owners must cancel and join their own scheduling tasks before source close.

Decoded receivers own bounded native queues: one latest video frame or ten PCM frames. `PeerPort::media_stats` drains at most those bounds, returning scalar RTP counters and sampled content signatures/PCM squared-sample energy. Counters are native RTP statistics; `observed_frames` counts only drained samples. Observations are diagnostic and do not identify publication frame boundaries. No raw media or per-frame event enters the runtime control queue or JSON IPC. Peer close synchronously removes sinks and clears queues, with no receiver task to await.

The binding is locally vendored at the same published versions. [libwebrtc patch provenance](../../vendor/libwebrtc/PV_PATCHES.md) and [webrtc-sys patch provenance](../../vendor/webrtc-sys/PV_PATCHES.md) record exact upstream commit and cached crate archive hashes, with verbatim upstream LICENSE and notices. Local changes expose the existing direction setter, make absent tracks/current direction safe, and provide a raw video constructor without an internally detached keepalive. Global Cargo registry sources are never modified.

The native media integration test proves actual bidirectional decoded synthetic video/audio, attach/detach/republish without SDP, stable MIDs, leave/rejoin, and joined producer teardown. It also directly regresses the null sender/direction cases. A separate macOS lifecycle integration binary warms process globals, proves a retained sink keeps native runtime threads alive, and repeats decoded success, admission/publication rejection, handshake cancellation and partial initialization. It samples macOS threads/FDs, requires at least three active native threads to disappear, and checks bounded return to a nonincreasing idle baseline. This observes native runtime/handle-graph teardown; it is not proof that every allocation is leak-free. Physical camera/microphone/screen capture, speaker playout, rendering and target-platform capability claims require their own adapters and acceptance evidence.

## Toolchain and artifact

Verified local compilation: Rust 1.88.0, Xcode 26.5, macOS arm64. Cargo.lock pins `webrtc-sys 0.3.47`, `webrtc-sys-build 0.3.19`, and the transitive graph. The current build helper downloads this release once into Cargo's scratch cache:

- Release: [webrtc-89d790b](https://github.com/livekit/rust-sdks/releases/tag/webrtc-89d790b)
- Archive: `webrtc-mac-arm64-release.zip`, 277,148,750 bytes (about 264 MiB)
- **Published GitHub asset digest**, not independently rehashed downloaded bytes: `sha256:9f25fea48588deac18d68e120d18b33af7ef10f921f74b7c57f66b642262c348`
- Observed unpacked Cargo scratch artifact: approximately 1.1 GiB.

The upstream downloader uses HTTPS and removes its ZIP after extraction; this run used that downloader and did not independently verify the ZIP digest. Cargo verifies Rust crate checksums; that does not verify a native release asset. A release build should retain/checksum its downloaded archive and use `LK_CUSTOM_WEBRTC` for the verified extraction, or build the pinned engine itself. No full Chromium source build is required for the development gate.

The cache path is `target/debug/build/scratch-*/out/livekit_webrtc/livekit/mac-arm64-release-webrtc-89d790b/mac-arm64-release`. `licenses/libwebrtc-mac-arm64-LICENSE.md` is copied verbatim from this actual artifact's `LICENSE.md`; `licenses/webrtc-sys-NOTICE.md` is copied from the pinned crate. Preserve these notices in binary distributions. Engine sources derive from the upstream `m150_release` build described by the binding's pinned source.

Every final macOS executable/test host must link Objective-C categories with `-ObjC`. `media-native/build.rs` covers that crate's tests; the Tauri host owns its final link flag. Linux native builds require current upstream C++20/hermetic-libc++ tooling (clang 21+ and GLib development headers); the existing Ubuntu signaling jobs do not enable this adapter. Mobile, Windows and cross-device native operation are not verified by the macOS gate.

[Pinned upstream build helper](https://github.com/livekit/rust-sdks/blob/libwebrtc/v0.3.50/webrtc-sys/build/src/lib.rs) · [raw binding manifest](https://github.com/livekit/rust-sdks/blob/libwebrtc/v0.3.50/libwebrtc/Cargo.toml)
