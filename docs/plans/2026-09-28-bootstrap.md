# ParentView Next bootstrap implementation plan

**Goal:** Create a new repository with enforced layer boundaries, a separately buildable role-neutral media SDK, a real browser mesh laboratory, Tokio signaling and a Tauri host, while keeping ParentView roles exclusively in the service package.

**Architecture:** Client → Controller → adapter contracts; concrete adapters are constructed outside the core. ParentView services own roles/consent; the signaling service owns authenticated room membership. Every resource has one owner and a teardown contract.

**Tech Stack:** strict TypeScript, RxJS, Bun, Biome, Vite, Rust/Tokio/Axum, Tauri 2, coturn.

## Tasks

- [ ] Root workspace: Git feature branch, package/Cargo workspaces, locked dependencies, architecture checks and documentation.
- [ ] SDK: implement the public API in `docs/protocol.md`, DOM-free ports, readonly streams, mesh peer lifecycle and browser adapters. Verify late completion after leave, peer departure, repeated teardown and four-peer data/media exchange.
- [ ] Server: implement authenticated routes and wire contract in `docs/protocol.md`; test room capacity, forged identity/routing, token validation and disconnect cleanup. Configure bounded queues, timeouts and expiring state.
- [ ] Product services: explicit `parent`/`child` roles, separate session/screen/control approval, receiver-owned exclusive control grants, Network/Device ports and lifecycle tests. Core must import no adapter or platform package.
- [ ] Laboratory/Tauri: render live local/remote streams and connection status using the public SDK; expose actual native capability status. Build the host on macOS; do not claim native media implementation.
- [ ] Validation: package export/type checks, architecture boundary tests, Bun tests, Rust tests/clippy, desktop build, real browser multi-peer integration and screenshot inspection.

## Completion criteria for this bootstrap

Independent SDK artifacts exist; no ParentView role identifiers are in SDK source. ParentView service tests demonstrate explicit role policy and consent without media-engine imports. Signaling authenticates sender identity and room admission. Browser peers exchange actual WebRTC tracks/data. Repository documents which native/production capabilities are still pending.

## Rulings

- Repository directory is `parentview-next`; create local Git history first. Remote publication is a separate delivery action.
- Initial runtime verification uses browser WebRTC, an allowed open-source/built-in engine, because four native capture/render adapters require their own milestone. Tauri host capability reporting is honest about this boundary.
- Monorepo source organization does not couple SDK releases to ParentView; SDK has its own exports, build and contract tests.
