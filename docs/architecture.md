# Architecture and ownership

The media SDK is a reusable, role-neutral product. ParentView is one consumer. A peer is a transport participant, a source is a camera/screen/microphone track, and a data channel carries application data. Neither a peer nor a source has a parent/child role.

## Dependency direction

```mermaid
flowchart TD
  UI[Presentation] --> App[ParentView service: roles, consent, control]
  App --> Client[MediaClient: public contract]
  Client --> Runtime[MediaController: state and lifecycle]
  Runtime --> Ports[Engine and signaling interfaces]
  Browser[Browser adapters] -. implement .-> Ports
  Native[Native adapters: next milestone] -. implement .-> Ports
  Composition[Composition root] --> UI
  Composition --> Browser
```

The Tokio signaling service is an independent authenticated rendezvous service. coturn relays encrypted WebRTC packets; it does not own rooms or product roles.

## Module contracts

| Module | Responsibility | Non-responsibility | State/resources | Dependencies | Teardown |
| --- | --- | --- | --- | --- | --- |
| SDK core | Peer lifecycle, mesh coordination, media events | UI, roles, concrete engines | Session generation, peers, owned tracks, Rx subscriptions | RxJS, own ports/types | Leave closes all peers and capture; destroy completes streams |
| Browser adapter | RTCPeerConnection, capture and WebSocket IO | Product policy | Native browser handles, listeners | Browser APIs and SDK ports | Close PC/socket; remove listeners; abort pending work |
| ParentView service | Local role, explicit consent and exclusive control grant | Wire protocol and actual OS input | Help-session state and consent | Narrow media/input ports, RxJS | End revokes grants and releases session resources |
| Network service | Environment network observations | Declaring a peer connected | Environment listeners | Environment port | Remove listeners and complete streams |
| Device service | Device catalog and capabilities | Family members/relationships | Device snapshots and refresh work | Device catalog port | Cancel/ignore pending refresh and remove listeners |
| Signaling server | Device credentials, bounded rooms, membership and signal routing | Media packets, roles, capture | Expiring credentials/rooms, per-socket membership | Tokio, Axum | Disconnect removes member and notifies remaining peers |
| Desktop host | Tauri lifecycle and native capability report | Product authorization or invented media availability | App lifetime | Tauri, platform APIs | Native tasks stopped with host |

## State and trust

Only the actual media controller owns media state. In the browser laboratory this is TypeScript; a future native runtime owns native state and exposes a TS facade. These implementations are alternatives, never two concurrent authorities for the same session.

Device token authenticates an installation. A room invitation secret admits that device into a media room. Media-room admission does not grant remote-input authority. ParentView consent/control grants are checked separately at the receiving service. An incoming data message must never directly invoke native input.

ParentView exposes `parent` and `child` explicitly in the service domain. SDK functionality is capability-based: publication, subscription, capture, transport, statistics. ParentView policies compose these features without inheritance from role-specific SDK clients.

## Delivery sequence

1. Repository, boundaries, contracts, role-neutral browser mesh, authenticated Tokio signaling, TURN configuration and product service tests.
2. Native libwebrtc integration and OS capture/render adapters, validated separately on every target. Raw frames remain native.
3. Persistent authenticated device pairing, product approval UI, push lifecycle, OS remote-input adapters and exclusive control grants.
4. Cross-device performance, forced relay, network transition, permission-revocation and repeated-lifecycle acceptance tests.

Initial laboratory verification does not substitute for steps 2–4. Native capture, mobile delivery and remote control remain explicit unfinished work until verified.
