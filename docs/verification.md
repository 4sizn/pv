# Prepublication verification — 2026-09-28

This report covers the first repository milestone: a role-neutral browser media SDK, ParentView service contracts, authenticated signaling and a Tauri laboratory host. It does not certify the finished cross-platform product.

Environment: macOS arm64, Node.js 22.19.0, Bun 1.1.24, Rust 1.88.0, installed Google Chrome, Playwright 1.63.0 and its Chromium 153.0.8010.12. `CI=true` selects downloaded Chromium on this same Mac; it does not reproduce a hosted Linux runner.

## Passed

| Check | Evidence |
| --- | --- |
| `bun install --frozen-lockfile` | Locked dependencies install without changing either lockfile |
| `bun run verify:all` | Complete gate exits successfully: architecture, types, style, Bun tests, isolated SDK package, Rust formatting/tests/Clippy, web build and Chrome E2E |
| TypeScript contracts | SDK core compiles without DOM types; architecture checks pass for 22 source files; strict TypeScript and Biome pass; 60 tests / 256 assertions pass |
| SDK package surface | Actual npm tarball runs in an isolated consumer with only RxJS; core/browser ESM exports load without DOM globals; readonly, DOM-free public types and lifecycle/teardown contracts pass |
| Rust signaling | 22 tests pass, including real HTTP/WebSocket authentication, authoritative sender identity, room isolation, four-peer capacity, origin checks, timeouts and teardown |
| Rust host | Capability test passes; unfinished native adapters remain reported as unsupported |
| Rust quality | `cargo fmt --all --check`; strict Clippy passes for signaling and desktop targets |
| Browser production build | Vite build succeeds; E2E runs against the built preview, not a development server |
| Browser E2E | 5 tests pass in installed Chrome and again with `CI=true bun run test:e2e`: adapter rollback/publication lifetime, invitation recovery, four-peer mesh, mobile width and renderer handle replacement |
| Intermittent-failure regression | After the adapter fix, the four-peer Chromium scenario passes five consecutive additional runs, with automatic retries disabled |
| `bun run verify:desktop` | Host capability test, desktop Clippy and Tauri debug executable build all pass |

The four-peer test creates four separate browser contexts with independent server-issued credentials. All four publish camera and microphone, receive the other three participants, and send messages to all three. It asserts advancing decoded video frames and received audio bytes. It also checks simultaneous camera off/on, a second video source per participant, removal of that source, participant departure/rejoin/republication and final teardown.

Camera inputs are generated test devices. Microphones use the checked-in synthetic tone, so each capture has continuous input independent of Chromium's default shared beep state. Audio assertions require new received bytes on every live connection. The second video source is a canvas stream injected into the screen-capture call. These tests exercise real WebRTC encoding, transport and playback; they do not exercise physical capture devices, an OS screen chooser or actual screen capture permissions.

## Defects exposed by the regression checks

- Invitation tests failed on empty/malformed ICE URLs and incomplete TURN credentials. Validation now rejects them before device registration; a browser test verifies the visible error, idle state and successful creation of a new room afterward.
- Layer checks previously allowed Node imports, import-type references, backtick role literals and relative service imports into private SDK modules. Negative fixtures now reject them while preserving local ports, RxJS and the public SDK entry.
- The four-peer test exposed an intermittent video freeze. A minimal native-engine reproduction and a test of the actual browser adapter isolated an answer-time reused transceiver followed by a later offer rollback. Both explicit and implicit rollback reproduced the problem. Dedicated publication transceivers fix it without changing core negotiation policy. Removal stops the owned transceiver, preserves the capture track and allows m-line recycling; eight completed publish/remove cycles keep a constant SDP section count.
- The renderer ignored a changed native playback handle when peer/source IDs stayed the same. The regression fails against the previous renderer and passes after rebinding the existing media element. It also verifies track ownership, DOM cleanup and suppression of delayed autoplay notices after destruction.
- The isolated package gate was checked negatively: temporarily removing built exports caused the consumer check to fail; the original built output was restored.

The initial bootstrap also fixed sender/receiver native track-ID mismatch by correlating source metadata with negotiated MID. Existing unit and browser checks continue to cover this behavior. The first intermittent fake-audio count failure was not established as an SDK defect; the continuous audio fixture and stronger progress assertion remove dependence on default synthetic-input state.

## Visual evidence

Generated images live in ignored `proof/` and are inspected separately from test assertions:

- `four-peer-mesh.png`: 4/4 membership, local camera and second source, six remote video streams, three remote audio controls and messages from all participants. The invitation secret is redacted before capture.
- `mobile-laboratory.png`: 390px browser layout. This is a responsive viewport check, not an Android/iOS device test.
- `invalid-invitation.png`: rejected synthetic TURN invitation, visible validation error, no registered device and enabled retry/create controls. Inspected directly and opened in macOS Preview; reproduce with `bun run test:e2e e2e/invitation.spec.ts`.
- `tauri-macos.png`: actual packaged macOS host rendering its local capability status, captured during the earlier bootstrap. This prepublication gate builds the debug executable without packaging.

The earlier bootstrap also passed required-secret Compose configuration validation. Docker's daemon was unavailable, so no TURN session was established.

## Still unverified or unimplemented

- Native libwebrtc media/capture/render adapters, physical-device performance, Android/iOS/Windows builds and OS remote input.
- Long-lived authenticated device pairing, distributed help approval and the actual parent/child product UI. Current service policy tests use injected ports and trusted participant records.
- TURN traversal, public Internet/cross-device networking and Docker relay address mapping. Docker's daemon was unavailable; [the container template needs a tested relay topology](../infra/README.md).
- Automatic signaling recovery, ICE restart and network changes. Current disconnect handling ends the media session.
- Hosted CI execution and repository rulesets. Four jobs and the aggregate **Required checks** are prepared; the aggregate condition was exercised against every success/failure/cancellation/skip combination. YAML/action inputs were checked against official tagged actions. The workflow skill's `ci_monitor.cjs` helper was absent, so that helper's action-version check could not run. No GitHub remote or CI run was created.
- Production enrollment/rate controls, durable credentials/storage and deployment configuration.

Git history is local on `codex/bootstrap`. The destination is `4sizn/pv`; no code was pushed and no branch-protection setting was changed. See [the repeatable checks](testing.md). No old ParentView data or runtime compatibility is required.
