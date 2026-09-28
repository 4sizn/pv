# ParentView vendored binding patches

Upstream crate: `libwebrtc 0.3.50`.
Repository: https://github.com/livekit/rust-sdks/tree/2dd762da4fdc8b73504983aff078d7824ea5d4ee/libwebrtc
Upstream crate archive SHA-256: `26fe8fba1d9ffa998f7add0e223f2b62e2726840f9722ca39f3d68ae30a83d9d` (computed from locally cached crates.io archive).
Source copied from that exact installed crate; Cargo download marker/checksum metadata omitted.
The upstream repository Apache-2.0 LICENSE is included verbatim; original file headers and bundled notices are preserved.

This is the raw WebRTC binding, not the LiveKit application/media client SDK.
Local changes are bounded to native safety and ownership requirements; see the repository diff.

## Local changes

- Expose existing native transceiver direction setter for answerer slot adoption.
- Add raw video-source constructor without detached keepalive; frame producer is caller-owned.
