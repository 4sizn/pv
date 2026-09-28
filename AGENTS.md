# ParentView Next — engineering rules

## Binding product decisions

- This is a new project. The previous ParentView is deprecated reference material. No legacy migration or LiveKit compatibility.
- Target Android, iOS, Windows and macOS. Do not claim a platform capability until its actual adapter is implemented and tested.
- Four participants maximum in the first release; every participant may publish screen, camera and audio. Use peer-to-peer mesh with coturn relay when needed.
- Open-source WebRTC engines are allowed; complete third-party media SDKs and hosted media platforms are not.
- The media SDK MUST NOT contain ParentView roles, pairing relationships, help-session approval, or product authorization rules. `parent` and `child` belong exclusively to the ParentView application/service domain.
- No account signup in the initial product. Devices still require authenticated identity; possession of a display name or peer ID is not authentication.

## Layer responsibilities are a rule

Every module must document responsibility, non-responsibility, owned state/resources, allowed dependencies, public contract, and teardown.

| Layer | Owns | Must not own |
| --- | --- | --- |
| Presentation | Rendering, UI-local state, forwarding user intent | Media recovery or product authorization |
| ParentView application/service | parent/child, relationship, consent and control grants | SDP/ICE or OS calls |
| SDK client | Public typed commands and readonly observables | Product roles or duplicate lifecycle state |
| SDK controller/runtime | Media state transitions, peer negotiation, recovery, track lifetime | ParentView policy or concrete browser/OS imports |
| Adapter | One concrete engine/resource operation and native event conversion | Independent product/retry policy |
| Composition root | Concrete dependency construction and lifetime wiring | Domain decisions |

- Depend on interfaces. Core must never import concrete adapters. Concrete assembly lives in composition roots.
- One authoritative state owner per running resource. A Rust-backed client must not recreate the native state machine in TypeScript; a browser controller owns browser resources in the browser validation adapter.
- Constructor injection, cohesive classes and composition; introduce abstract classes only when there is shared implementation.
- Observable streams use `$`. Keep Subjects private; expose only Observables. Separate snapshots from events. Recoverable errors are values, not terminal Subject errors.
- Commands return promises when completion matters. Capture, peer, listener, timer and task owners must implement deterministic teardown. Closing a session is reusable; destroying its owner is terminal and idempotent.
- Serialize SDP/ICE mutations per peer. Cancellation of an Rx subscription is not cancellation of an arbitrary promise or native operation.
- Lifecycle and recovery policy belong to controllers. Adapters never start their own competing retry loop.
- Roles are not credentials. Session admission and remote-input authorization must be checked at their authoritative service/receiver boundaries.
- Never route raw video frames through JSON IPC. Media stays in the engine/OS pipeline.
- Test architecture boundaries and contracts, not just happy-path output.

## Reference style

Use `4sizn/ws-client-pack` commit `ceb533ad100acacd83f125a9789da3d528dae27b` as a design reference: Client → Controller → Adapter, explicit lifecycle, readonly Rx streams, injected dependencies and contract tests. Do not copy its implementation blindly.

TypeScript: strict, Biome, spaces/2, double quotes, line width 100, trailing commas all; Bun tests. Rust: rustfmt/clippy, owned asynchronous tasks and bounded channels. Do not add compatibility aliases or speculative manager hierarchies.

## Work and evidence

- Read `docs/architecture.md` and `docs/protocol.md` before changing interfaces.
- Keep roles outside `packages/media-sdk`. New entry points may not leak browser/Tauri globals into the SDK core.
- Use feature branches with the `codex/` prefix. Do not publish, deploy or create credentials in documentation.
- Never claim the native engine, remote input, TURN traversal or mobile behavior works based only on browser or fake-adapter tests.
- Run the documented checks and update the implementation status honestly. Browser laboratory is a developer verification surface, not the finished ParentView product.
