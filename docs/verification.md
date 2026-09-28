# Bootstrap verification — 2026-09-28

This report covers the first repository milestone: a role-neutral browser media SDK, ParentView service contracts, authenticated signaling and a Tauri laboratory host. It does not certify the finished cross-platform product.

Environment: macOS arm64, Node.js 22.19.0, Bun 1.1.24, Rust 1.88.0, installed Google Chrome, Playwright 1.63.0.

## Passed

| Check | Evidence |
| --- | --- |
| `bun run check` | SDK core compiles without DOM types; architecture checks pass for 22 source files; strict TypeScript and Biome pass; 35 tests / 103 assertions pass |
| SDK package surface | Core and browser ESM entry points load in Node without DOM globals; `npm pack --dry-run --json` contains 20 package files with independent JS/type declarations and RxJS as a peer dependency |
| Rust signaling | 22 tests pass, including real HTTP/WebSocket authentication, authoritative sender identity, room isolation, four-peer capacity, origin checks, timeouts and teardown |
| Rust host | Capability test passes; unfinished native adapters remain reported as unsupported |
| Rust quality | `cargo fmt --all --check`; strict Clippy passes for signaling and desktop targets |
| Browser production build | Vite build succeeds; E2E runs against the built preview, not a development server |
| Browser E2E | 2 tests pass: real four-peer mesh lifecycle and a 390px responsive layout without horizontal overflow |
| Tauri macOS | Debug `.app` builds, launches and renders the laboratory with the host capability response |
| Compose | Required-secret configuration passes `docker compose ... config --quiet`; this validates configuration syntax only |

The four-peer test creates four separate browser contexts with independent server-issued credentials. All four publish camera and microphone, receive the other three participants, and send messages to all three. It asserts advancing decoded video frames and received audio bytes. It also checks simultaneous camera off/on, a second video source per participant, removal of that source, participant departure/rejoin/republication and final teardown.

Camera/microphone inputs are Chrome's generated test devices. The second video source is a canvas stream injected into the screen-capture call for this test. The test exercises real WebRTC encoding, transport and playback; it does not exercise physical capture devices, an OS screen chooser or actual screen capture permissions.

Browser verification exposed a real interop defect: receiver track IDs need not equal sender track IDs. Source metadata now uses negotiated transceiver MID, and receiver handles survive unpublish until peer teardown so renegotiation can reuse them. Unit and browser regression tests cover this behavior.

## Visual evidence

Generated images live in ignored `proof/` and are inspected separately from test assertions:

- `four-peer-mesh.png`: 4/4 membership, local camera and second source, six remote video streams, three remote audio controls and messages from all participants. The invitation secret is redacted before capture.
- `mobile-laboratory.png`: 390px browser layout. This is a responsive viewport check, not an Android/iOS device test.
- `tauri-macos.png`: actual packaged macOS host rendering its local capability status.

## Still unverified or unimplemented

- Native libwebrtc media/capture/render adapters, physical-device performance, Android/iOS/Windows builds and OS remote input.
- Long-lived authenticated device pairing, distributed help approval and the actual parent/child product UI. Current service policy tests use injected ports and trusted participant records.
- TURN traversal, public Internet/cross-device networking and Docker relay address mapping. Docker's daemon was unavailable; [the container template needs a tested relay topology](../infra/README.md).
- Automatic signaling recovery, ICE restart and network changes. Current disconnect handling ends the media session.
- Hosted CI execution. The workflow is prepared and its YAML/action inputs were checked against official tagged actions. The workflow skill's `ci_monitor.cjs` helper was absent, so that helper's action-version check could not run. No GitHub remote or CI run was created.
- Production enrollment/rate controls, durable credentials/storage and deployment configuration.

Git history is local on `codex/bootstrap`. No old ParentView data or runtime compatibility is required.
