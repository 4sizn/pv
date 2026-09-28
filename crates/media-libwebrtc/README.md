# Native engine adapter

Owns libwebrtc factory, peer and data-channel handles and callback conversion. Implements `pv-media-runtime` ports; does not own rooms, product roles, retry policy, Tauri IPC, or OS capture. Native WebRTC owns its network, worker and signaling threads; the Rust session actor owns when handles are created, configured and released.

The engine dependency is the raw `libwebrtc = "=0.3.50"` binding. The `livekit` room/client SDK is not a dependency. Peer callbacks enqueue bounded owned data, never await or close/reconfigure a peer while native callback mutexes are held. The adapter clears observers before closing channels and connections. Drop is also idempotent. A data send requires an open channel, <=16 KiB UTF-8 payload, and <=1 MiB queued sender bytes.

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
