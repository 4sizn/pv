# Prepublication regression baseline

**Goal:** Make the existing bootstrap's contracts executable before publishing to `4sizn/pv`.

**Architecture:** Retain Bun fake-port contract tests, Cargo HTTP/WebSocket integration tests and Playwright real browser transport tests. Package verification consumes the npm tarball outside the workspace. CI reports one stable aggregate check; it does not claim unimplemented native media or TURN support.

**Tech stack:** Existing Bun, TypeScript, RxJS, Cargo, Playwright and GitHub Actions; no new test dependencies.

- [x] `packages/media-sdk/tests/controller.test.ts`: verify unexpected signaling loss, failed-peer isolation, invalid-command ownership, throwing teardown adapters and failed negotiation recovery.
- [x] `apps/desktop/test/invitation.test.ts`: verify fragment-only invitation roundtrip/consumption and malformed input rejection; fix only confirmed validation defects in `src/invitation.ts`.
- [x] `scripts/architecture.test.ts`: prove that Node imports, TypeScript import types and role literals written with backticks cannot bypass layer checks; keep legal composition working.
- [x] `scripts/check-sdk-package.ts`: pack built output, consume it in an isolated temporary directory with only RxJS, load core/browser exports in Node, verify core lifecycle and compile a DOM-free consumer with read-only streams.
- [x] `package.json`: include application tests and provide `check:sdk-package`, `verify`, `verify:all` and `verify:desktop` commands with nonzero exit on any failing step.
- [x] `playwright.config.ts`: fail on focused tests, use installed Chrome locally and downloaded Chromium in CI, keep one worker and no retries, and avoid traces containing credentials.
- [x] `.github/workflows/ci.yml`: run types/package, Rust signaling, real four-peer E2E and macOS host checks; `Required checks` passes only when every job succeeds.
- [x] Retain browser regressions for the confirmed rollback sender stall and same-source playback-handle replacement; prove failure before fixes and success after fixes.
- [x] Run the complete local gate, exercise the CI browser configuration, record exact test counts and remaining environment limits, and commit the regression baseline locally.

The destination repository is currently empty. This task prepares and verifies the local code; no GitHub publication or branch-protection change is included.
