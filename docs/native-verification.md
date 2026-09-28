# Native media transport verification — 2026-09-28

The native Rust SDK now transports one camera, screen and microphone source per participant through fixed negotiated slots. Source creation/capture is injected; this milestone proves generated I420 video and PCM transport, not physical device capture or native rendering. The Rx facade is `NativeMediaClient` and exposes bounded, frozen source metadata alongside its existing data/session operations.

## Native media evidence

- Two actual native peers use the authenticated signaling service, raw libwebrtc, software VP8 video and native audio encoding/decoding on macOS arm64.
- Initial negotiation has empty camera, screen and microphone slots. Both sides agree on actual MIDs; receiving RTP/video/audio counters start at zero.
- Generated camera and screen frames have distinct changing luma; generated PCM has nonzero energy. Receive checks use decoded native samples plus RTP counters, rather than metadata or sender counters alone.
- Unpublish joins the source producer, clears its remote manifest and stops incoming RTP. Republish reuses the MIDs with a new source ID and resumes decoded media without another SDP exchange.
- Leave/rejoin creates a new peer lifetime and resumes video. The proof counts producer joins so successful teardown cannot conceal detached feed tasks.
- Direct raw-binding regressions exercise an absent sender track, `set_track(None)`, an unnegotiated current direction, and input rejection after source close/drop.
- A standalone macOS resource test warms the native paths and observes threads and file descriptors across 12 decoded-media, rejection, cancelled-handshake and partial-initialization cycles. Every cycle returned to 11 file descriptors; idle threads declined from 10 to 8 as helper pools retired, and each cycle released at least three native threads. Its retained-sink positive control held 13 threads / 13 descriptors until release, then returned to 10 / 11. This establishes bounded resource cleanup for these workloads, not allocation-level leak freedom or other-platform behavior.

Local aggregate checks passed: 104 TypeScript tests / 694 assertions, strict types, Biome, architecture, standalone DOM-free package, and 63 Rust tests: signaling 22, runtime 27 lifecycle tests plus its architecture guard, native signaling 4, real native media 2, real native data/admission 2, native resource cleanup 1, and Tauri host 4. Cargo formatting, strict native/host Clippy, production web build, the Tauri debug executable and all 5 browser E2E tests passed. Local versions: Node.js 24.19.0, Bun 1.1.24, Rust 1.88.0, Xcode 26.5 on macOS arm64.

The first host build exhausted local disk while writing a generated static archive. Obsolete generated archives/build directories were identified by registry-source provenance and removed; the retained engine prebuilt and current vendored outputs were preserved. The complete host gate then passed. This was an environment failure, not a source/test failure.

Hosted verification at the published commit is reported in the task completion message. These local results do not substitute for that hosted run.

## Review findings addressed in this slice

1. Upstream nullable track operations dereferenced empty tracks. Narrow vendored guards now match the Rust Option contracts; direct native tests cover them.
2. Video input previously borrowed a mutable native buffer that the engine could retain. Our ingress consumes the initialized frame to establish ownership.
3. The default raw video source starts a detached keepalive producer. A vendored constructor disables it; source scheduling and cancellation stay with the explicit producer owner.
4. A malformed source kind failed deserialization of the whole signaling message. The adapter now separates the authenticated envelope from peer content so the runtime can close only that peer.
5. Nested or overlapping stop operations could overwrite a source cleanup error with an empty successful teardown. Regression coverage requires cleanup errors to survive the final stop acknowledgement.

Physical capture, microphone/speaker device selection, screen permission/picker behavior, native presentation, TURN/Internet transport, four-peer native media load and non-macOS builds remain unverified. The packaged UI still exposes its native **data** probe and keeps `nativeMedia: false`.

---

# Prior native data milestone — 2026-09-28

This slice adds a role-neutral native data session to the existing repository. It does not implement native camera, microphone, screen capture, rendering or OS input.

## Ownership and public surface

- `pv-media-runtime` owns Tokio session/membership/negotiation state through injected ports.
- `pv-media-libwebrtc` owns raw engine resources and converts native callbacks into bounded owned events.
- `pv-media-native` composes that runtime with authenticated WebSocket signaling.
- `@parentview/media-sdk/native` validates and projects Rust snapshots/events through readonly Rx streams. It does not construct the browser controller or import Tauri.
- The desktop app implements Tauri transport and document/window resource ownership. ParentView roles remain exclusively in the service domain.

## Verification status

The following historical checks covered the earlier data slice on macOS arm64. Its exact commit `8a8516c` subsequently passed every hosted job: [native data CI run](https://github.com/4sizn/pv/actions/runs/36408930264).

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
