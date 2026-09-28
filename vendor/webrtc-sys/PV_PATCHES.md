# ParentView vendored binding patches

Upstream crate: `webrtc-sys 0.3.47`.
Repository: https://github.com/livekit/rust-sdks/tree/2dd762da4fdc8b73504983aff078d7824ea5d4ee/webrtc-sys
Upstream crate archive SHA-256: `434f9b6ba0e4609ff2729978e9e48318249625cd96e56ddc44016b2d18d47997` (computed from locally cached crates.io archive).
Source copied from that exact installed crate; Cargo download marker/checksum metadata omitted.
The upstream repository Apache-2.0 LICENSE is included verbatim; original file headers and bundled notices are preserved.

This is the raw WebRTC binding, not the LiveKit application/media client SDK.
Local changes are bounded to native safety and ownership requirements; see the repository diff.

## Local changes

- Guard null sender track on detach.
- Return null when converting an absent native media track, matching Rust Option.
- Guard absent current direction before optional access; return a caught bridge error instead of optional.value().
