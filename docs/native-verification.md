# Native data milestone verification — 2026-09-28

This slice adds a role-neutral native data session to the existing repository. It does not implement native camera, microphone, screen capture, rendering or OS input.

## Ownership and public surface

- `pv-media-runtime` owns Tokio session/membership/negotiation state through injected ports.
- `pv-media-libwebrtc` owns raw engine resources and converts native callbacks into bounded owned events.
- `pv-media-native` composes that runtime with authenticated WebSocket signaling.
- `@parentview/media-sdk/native` validates and projects Rust snapshots/events through readonly Rx streams. It does not construct the browser controller or import Tauri.
- The desktop app implements Tauri transport and document/window resource ownership. ParentView roles remain exclusively in the service domain.

## Verification status

The following completed checks cover the native slice on macOS arm64. The baseline hosted CI run is recorded separately; the native branch requires its own hosted result.

- Baseline hosted CI passed on `58cae49`: [GitHub run](https://github.com/4sizn/pv/actions/runs/36406077400).
- Rust 1.88.0 and Xcode 26.5 compiled the raw binding on macOS arm64.
- Native peers exchanged data through actual libwebrtc and the authenticated signaling server. The first four-peer run hit its readiness deadline; subsequent runs passed. Rejoin callback lifetime was later reproduced independently and fixed; the original timeout cannot be attributed conclusively without the missing original diagnostics. The fixed four-peer scenario then passed five additional consecutive runs with no automatic retry.
- Final TypeScript checks passed: 97 tests / 516 assertions, strict type checking, architecture checks on 27 source files, Biome, DOM-free SDK build and isolated npm-consumer verification.

- Rust runtime: 13 lifecycle tests plus the dependency-architecture test passed. Native composition: 3 signaling/task tests and 2 real-engine integration tests passed. Strict Clippy passed for all three native crates.

- Tauri host: 4 tests passed for capability reporting, ownership, bounded handles and document reload/stale-open rejection. Desktop Clippy and `cargo fmt --all --check` passed.
- Existing regression paths: signaling 22 tests and strict Clippy passed; `CI=true bun run test:e2e` passed all 5 browser tests with retries disabled.
- `bun run --cwd apps/desktop tauri build --debug --bundles app` produced a 52.65 MiB macOS app. Both bundled engine notices were byte-for-byte identical to their checked-in sources.
- The actual packaged Tauri app completed its native two-peer check through the Rx facade and invoke transport. After a home-link document navigation, the new document completed the check again. Both reported exact bidirectional delivery and successful leave/destroy. The app then exited normally.

## Visual proof and reproduction

`proof/native-data-macos.png` is the actual packaged macOS window after successful native delivery and cleanup. It was inspected directly and opened in macOS Preview. This is an ignored local artifact, not a committed test fixture. The native section is distinct from the idle browser-media controls above it.

Start `bun run dev:server`, build the debug app using the command above, launch `target/debug/bundle/macos/ParentView Mesh Laboratory.app`, and select **네이티브 데이터 검사 시작**. The success text is **네이티브 2피어 양방향 데이터 확인 · 자원 해제 완료**. Use the home link to navigate to a fresh document, then run the native check again. The deterministic host test additionally verifies queued stale-document opens are rejected; the visual run alone does not prove every cancellation interleaving.

## Regressions found during review

1. An old native peer callback could affect a replacement with the same peer ID after departure/rejoin. Per-peer incarnation checks supplement the session generation; a retained old fake sink provides a deterministic regression.
2. Cleanup failures/timeouts could be hidden by a successful terminal snapshot. Stop completion must propagate cleanup failure, and the concrete socket task must finish before reporting successful cleanup.
3. A late signal to a departed peer could terminate unrelated healthy peers. The runtime now treats the server's recoverable `peer-unavailable` result as an error value and preserves those peers.
4. JavaScript page exit alone could leave native clients alive after WebView reload. Host document invalidation and document leases reject queued old-document opens and release abandoned clients independently of renderer promises.

## Boundaries still unverified

Native media/capture/rendering, TURN/Internet and cross-device transport, physical-device performance, network transitions/recovery, Android/iOS/Windows native builds, durable enrollment and the finished parent/child service remain outside this data milestone. The raw engine's release digest and notice provenance are documented in [the adapter README](../crates/media-libwebrtc/README.md); the upstream downloader's removed ZIP was not independently rehashed. Binary bundles include the copied upstream notices.
