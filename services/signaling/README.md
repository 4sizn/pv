# Signaling service

Authenticated rendezvous for up to four peers per room. Run from repository root with `cargo run -p parentview-signaling`. The default listener is `127.0.0.1:8787`. HTTP/WS routes and camelCase payloads follow `../../docs/protocol.md`.

Use `POST /devices` once per installation, then `POST /rooms` with its `Authorization: Bearer <deviceToken>`. The creator shares the returned room ID and invitation token out of band. Each joining installation presents its own device token and the shared invitation token in its first WebSocket message. Tokens are bearer secrets; peer IDs and room IDs are not credentials. Server state retains only SHA-256 token digests and uses constant-time room-token comparisons.

The client must send `{"type":"ping"}` periodically (recommended every 20 seconds) while otherwise idle. A read deadline removes silent sockets. Outgoing delivery is bounded; consumers whose queue fills are disconnected. Signals have an opaque payload, an authenticated server-generated `from`, and a same-room recipient. Unknown envelope fields, including caller-supplied `from` or `peerId`, are rejected. A device can have only one active membership across the entire service.

## Configuration

Comma-separated URL/origin lists may be empty for ICE. No STUN or TURN endpoint is selected automatically. The application must receive the returned `iceServers` for new peer connections.

Packaged Tauri defaults are explicitly admitted: `tauri://localhost` for macOS and `http://tauri.localhost` for Windows. The only accepted non-HTTP origin is exactly `tauri://localhost`; arbitrary custom schemes, alternate Tauri hosts, paths and ports are rejected. Configuring `ALLOWED_ORIGINS` replaces the default list.

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `SIGNALING_BIND` | `127.0.0.1:8787` | TCP listen address |
| `ALLOWED_ORIGINS` | `http://localhost:1420,http://127.0.0.1:1420,tauri://localhost,http://tauri.localhost` | Exact browser and packaged Tauri origins; no wildcard |
| `STUN_URLS` | empty | Deployment-owned STUN URLs |
| `TURN_URLS` | empty | Deployment-owned coturn URLs |
| `TURN_SHARED_SECRET` | absent | Required if TURN URLs are configured; supply outside source control |
| `TURN_CREDENTIAL_TTL_SECONDS` | `3600` | TURN REST username expiration |
| `DEVICE_TTL_SECONDS` | `604800` | Absolute device credential lifetime |
| `ROOM_TTL_SECONDS` | `86400` | Absolute room invitation lifetime |
| `JOIN_TIMEOUT_SECONDS` | `10` | First-message authentication deadline |
| `HEARTBEAT_TIMEOUT_SECONDS` | `45` | Maximum gap between received WebSocket messages |
| `MAX_DEVICES` | `10000` | In-memory device capacity |
| `MAX_ROOMS` | `2000` | In-memory room capacity |
| `MAX_CONNECTIONS` | `1024` | Concurrent WebSockets, including pending joins |
| `SIGNAL_QUEUE_CAPACITY` | `64` | Pending messages per socket |
| `MAX_MESSAGE_BYTES` | `65536` | Maximum incoming WebSocket message/frame size |

Writes time out after five seconds. The process scans due expirations every 15 seconds, and each credential/membership operation also applies due expirations. Empty rooms retain their invitation until room expiry, allowing leave/rejoin. A restart removes all credentials and invitations. Expired devices/rooms cancel their sockets and remove membership. Native clients may omit `Origin`; bearer authentication still applies. Browser HTTP mutation and WebSocket upgrade routes reject unlisted origins.

TURN credentials use coturn's TURN REST scheme: expiration timestamp plus peer ID as the username, and Base64 HMAC-SHA1 as its password. The shared secret is never returned. Unit tests validate credential generation; a live coturn/forced-relay acceptance test is still required before claiming TURN traversal.

## Ownership and public contracts

| Module | Responsibility and contract | Non-responsibility | Owned resources / dependencies / teardown |
| --- | --- | --- | --- |
| `models` | Strict client envelopes and server responses | Authentication and payload interpretation | Data only; serde; no teardown |
| `config` | Validate environment and operational limits | Room/connection state | Immutable configuration and deployment secret; standard library |
| `ice` | Generate configured ICE server responses and ephemeral credentials | Relay operation or media routing | Stateless HMAC/base64; no teardown |
| `state` | `AppState`: issue credentials, create rooms, join/route/leave, expire records | Socket IO, media and product policy | Mutex-protected bounded store and connection semaphore; rand/SHA2/Tokio channels; leave/cleanup/disconnect_all release membership |
| `http` | `router`: HTTP authentication boundary, explicit CORS, WS admission | Membership authority or media interpretation | Axum/tower-http requests and owned socket permits; permits drop on close |
| `ws` | `serve`: one bounded socket lifecycle and strict messages | Credential issuance or product policy | Socket, outgoing receiver, deadlines and membership lease; lease Drop removes membership and broadcasts departure |
| `main` | Construct listener/router/state, own cleanup task and shutdown | Domain decisions | Tokio tasks/listener; shutdown disconnects sockets, clears stores and joins cleanup task |

Library embeddings must own periodic `AppState::cleanup` and call `disconnect_all` on shutdown, as the executable does.

## Validation and limits

`cargo test -p parentview-signaling` covers credentials, unauthenticated access, room secrets, the four-peer limit, duplicate membership, same-room routing, spoofed identity rejection, stale teardown, expirations, queue saturation, real HTTP/WebSocket exchange, close cleanup, deadlines, connection capacity and oversized messages. Run `cargo clippy -p parentview-signaling --all-targets -- -D warnings` as well.

This is a local laboratory service. Production work remains: TLS termination, persistent credential rotation/revocation, abuse/rate limiting beyond bounded storage/socket counts, operator monitoring, coordinated multi-instance state and real deployment TURN testing. Device enrollment is intentionally unauthenticated and capacity-limited, so an exposed deployment needs an enrollment/admission policy. Product consent and remote-input authorization are separate application/service responsibilities.
