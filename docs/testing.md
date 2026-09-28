# Regression checks before publication

## Commands

| Command | Contract it protects |
| --- | --- |
| `bun run check` | DOM-free SDK build, layer dependencies/role isolation, strict types, formatting and all Bun tests including the desktop invitation parser |
| `bun run check:sdk-package` | Built npm tarball works outside the workspace with only its RxJS peer dependency; core/browser/native exports load in Node; readonly core types compile without DOM globals; owned tracks stop once and streams complete |
| `bun run verify` | All checks above, Rust formatting, signaling and platform-free native runtime tests/Clippy and production web build |
| `bun run verify:all` | `verify` plus actual browser WebRTC and application interaction tests |
| `bun run verify:native` | Actual native four-peer data mesh and two-peer bidirectional video/PCM decoding; fixed-slot republish, source/task teardown, authentication, runtime cancellation/manifests and strict native Clippy; initially supported on macOS |
| `bun run verify:desktop` | Web build, native host ownership/capability tests, desktop Clippy and Tauri debug executable build; requires the target platform's development tools |

Commands stop with a nonzero exit code on the first failure. They do not push code. Run `bun install --frozen-lockfile` first and commit lockfile changes only when dependencies deliberately change.

## Protected behavior

- SDK core stays role-neutral and depends on ports/RxJS. ParentView services can use SDK public contracts but cannot import private SDK modules or concrete browser/Tauri/Node IO. Negative fixtures cover ordinary imports, re-exports, dynamic imports, TypeScript import types and dependency cycles.
- Rust crate dependency checks keep the runtime independent of concrete engine, host and product dependencies. The native Rx entry rejects browser/Tauri/Node dependencies and a second TypeScript session controller.
- Native tests use real libwebrtc peers and the actual authenticated signaling service. Generated native media must produce changing decoded camera/screen samples and nonzero PCM energy in both directions. Detach stops incoming RTP, republish preserves MIDs without extra SDP, data remains usable during media, and leave/destroy join every test producer. Injected runtime tests cover pending-operation cancellation, single-reader ownership, queue overflow, early ICE, source manifests and failed-peer isolation.
- A separate macOS test process uses `ps` and `lsof` to check bounded thread/file-descriptor cleanup over repeated success, rejection, handshake cancellation and partial initialization. A retained native sink is its positive control. This detects native ownership leaks in those workloads; it is not an allocation profiler or proof for other platforms.
- A disconnected signaling session releases publications and peer resources. A failed peer or malformed peer signal does not stop healthy peers. Browser failed publications preserve caller-owned capture; Rust `publish(id, Box<dyn SourcePort>)` consumes and closes rejected sources. Native queued source cleanup and overlapping stop failures must complete or report a cleanup error before stop acknowledgement. Recoverable errors are Rx values rather than terminal stream errors.
- ParentView role eligibility, screen consent and exclusive input grants remain separate gates. Existing service tests cover stale async work, reentrancy, caller mutation, revoked queued input, permission loss and cleanup retry.
- Invitations carry admission secrets in the URL fragment, are consumed once, reject malformed origins/ICE settings, and discard untrusted identity/role fields. TURN credentials must be complete before constructing a media client. Full ICE/URI validation still belongs to the browser engine.
- Rust real-socket tests reject forged sender identity, incorrect credentials/origins and cross-room routing; enforce four-peer capacity, timeouts and queue limits; release memberships on disconnect.
- Browser E2E checks four real peers, generated camera/microphone streams, decoded video progress, newly received audio bytes on every live connection, bidirectional data, simultaneous republishing, second video sources and leave/rejoin. It also checks mobile-width layout and invalid invitation recovery.
- The renderer must bind a changed native track handle even when the peer/source ID stays the same. Its browser contract checks DOM reuse, playback cleanup, suppression of late autoplay notices after destruction, and that the view never stops caller-owned tracks.
- A deterministic browser-adapter test checks camera encoding after an answer-time publication and later colliding offer rollback. It also checks that publication removal leaves caller-owned capture alive and that negotiated publication toggles recycle SDP media sections.

The standalone package check creates a temporary consumer and removes it in `finally`. Its compile checks use no browser/Node ambient types; `skipLibCheck` matches the SDK's treatment of RxJS dependency declarations. It is a public API/packaging check, not a substitute for the core's source compilation.

## Reproduce the CI browser path

```sh
bunx playwright install chromium
CI=true bun run verify:all
```

On a Linux machine without browser system libraries, use `bunx playwright install --with-deps chromium`. Local default runs use installed Google Chrome. Both use an isolated browser profile and generated test media; a physical camera is unnecessary. Microphones use the checked-in synthetic tone in `e2e/fixtures/`, avoiding the default fake device's shared one-shot beep state. Tests own ports 1420 and 8787 and refuse to reuse an unrelated server. One worker runs at a time, focused tests fail configuration, and automatic retries are disabled.

Raw traces/video/automatic screenshots are disabled because they can retain invitation or device credentials. The config also sets `PLAYWRIGHT_NO_COPY_PROMPT=1` to disable automatic failure-page ARIA snapshots. This was checked against the locked Playwright runtime and actual failure output: `error-context.md` may still contain assertion errors and test source, but has no automatic page snapshot. Media failure diagnostics whitelist state transitions and RTP counters. Explicit proof screenshots redact real invitation values and are ignored by Git. CI does not upload artifacts.

## CI and branch protection

`.github/workflows/ci.yml` runs on pushes and pull requests with read-only repository permission. Four jobs cover TypeScript/package/web, Rust signaling/runtime, Linux browser E2E and macOS native mesh/Tauri host. The stable aggregate job is named **Required checks** and succeeds only if all four jobs succeed, including when a dependency job fails, is cancelled or is skipped.

The baseline hosted run at commit `58cae49` passed all jobs ([run](https://github.com/4sizn/pv/actions/runs/36406077400)). The repository can require **Required checks** in branch protection/rulesets. Merely adding this YAML does not configure GitHub merge protection. Publishing a branch does not alter these settings.

macOS debug-host compilation does not prove native media works. Mobile/Windows delivery, native capture/input, durable pairing, TURN relay topology and Internet/network-change tests remain separate milestones in `docs/architecture.md`.

## Native reproduction

Run `bun run verify:native` on macOS with Rust 1.88 and Xcode development tools. The first build downloads the pinned raw engine prebuilt; allow disk space for the archive, extracted library and debug binaries. Linux CI runs the platform-free runtime without linking the engine. Native Linux compiler setup and Windows/mobile engine builds remain separate work.

For the actual IPC/Rx path, start `bun run dev:server`, then `bun run dev:desktop`, and select the native data check. Success requires both clients to receive the exact opposite peer message and finish leave/destroy. No browser RTCPeerConnection participates in this check. The browser media controls remain a separate validation path.

The media test uses initialized generated I420 frames and 48 kHz mono PCM; it does not request camera, microphone or screen permissions. Native source inputs consume owned frames and reject writes after close/drop. Scalar diagnostic samples stay in Rust. Physical capture/rendering, four-peer native media load, cross-device operation and forced TURN remain separate acceptance gates.
