# Mesh laboratory host

This is a developer validation surface for the independently usable, role-neutral media SDK. It is not the finished ParentView family/help-session experience.

## Module contract

- **Presentation (`LabView`)** renders SDK snapshots, DOM media playback and UI-local form/busy state. It owns DOM listeners, rendered media references and its notice timer. It does not decide media recovery, authorization or peer negotiation. Destruction removes listeners, timers and playback references.
- **Application intent (`LabController`)** forwards join, publish, unpublish, send and teardown commands through the public SDK. It owns subscriptions and the current client reference, pending UI request cancellation, publication IDs and local preview listeners. It does not mirror SDK lifecycle policy or SDP/ICE state. On exit it aborts pending capture, destroys the client and unsubscribes.
- **HTTP adapter (`LaboratoryApi`)** registers each tab with the signaling service and creates authenticated rooms. It owns a memory-only device credential per server origin; never localStorage/sessionStorage/cookies. Its caller owns request cancellation. Destruction clears identities.
- **Capture adapter (`BrowserCapture`)** performs one browser camera/microphone or screen acquisition. A late permission result after cancellation is stopped. The caller owns captures until successful SDK publication; the SDK then owns track teardown. No recovery loop or role policy.
- **Composition root (`main.ts`)** constructs the concrete HTTP, browser capture and browser SDK adapters. Concrete native-track conversion is injected into the view/controller. It reports the Tauri host's actual capabilities and owns page-exit cleanup.
- **Tauri host (`src-tauri`)** owns window/application lifetime and the `native_capabilities` command. It reports `nativeMedia: false`, `remoteInput: false`, `browserValidationOnly: true` on every platform. It has no native media engine, OS remote-input command or product authorization policy.

## Run

From the repository root, start `bun run dev:server`, then `bun run dev`. Open `http://localhost:1420`, create a laboratory and paste its invitation into another tab. Each tab obtains an independent server-issued device identity. Invites carry room admission and ICE configuration in the URL fragment; consuming an incoming link removes the fragment from the address bar. Credentials are not added to queries, logged or persisted.

Both tabs can publish camera + microphone, publish screen capture where the browser supports it, render received media and exchange text. Browser autoplay restrictions may require pressing the received media's play control. Leaving closes the SDK, stops its publications, clears playback and unsubscribes the UI. Browser permissions and hardware must exist for real capture. The laboratory never fabricates peers, media, statistics or native capabilities.

`bun run build:web` builds the browser assets. `bun run dev:desktop` runs the Tauri shell. `bun run build:desktop` builds the SDK and an unbundled release executable. Rust 1.88+ and platform prerequisites are required. The `#[cfg_attr(mobile, tauri::mobile_entry_point)]` hook permits future mobile host integration; it is not evidence that Android/iOS capture, rendering or delivery works.

Native libwebrtc, native screen/camera capture, remote input, production credential persistence and mobile validation are separate unfinished milestones. The Tauri CSP allows only the local laboratory signaling server; other deployments require deliberate secure origins/CSP configuration. Without configured TURN, successful local/LAN tests do not establish Internet traversal.

UI test hooks include `create-room`, `invitation-input`, `join-room`, `invitation-output`, `media-state` (with `data-state`), `toggle-camera`, `toggle-screen`, `local-video`, `remote-video`, `message-input`, `send-message`, `sent-message`, `received-message`, and `leave-room`.
