# Laboratory signaling contract v1

All JSON uses camelCase. SDK and signaling payloads have no product roles. Default server binds `127.0.0.1:8787`; UI is `http://localhost:1420`. Production requires TLS, durable device credentials, admission/rate limiting and explicit origins.

## HTTP

- `GET /health` → `{ "status": "ok" }`.
- `POST /devices` → `{ "peerId": string, "deviceToken": string }`. Cryptographically random IDs and secret token. Token authenticates subsequent requests; never accept a client-selected peer ID as identity. Initial credentials are in-memory, bounded and expiring.
- `POST /rooms`, `Authorization: Bearer <deviceToken>` → `{ "roomId": string, "roomToken": string, "iceServers": [{ "urls": string[], "username"?: string, "credential"?: string }] }`. Random invitation secret; maximum four peers. Room secrets are not room IDs and must never appear in server logs.

## WebSocket `/ws`

First message, before any signal, within a bounded timeout:

```json
{"type":"join","deviceToken":"…","roomId":"…","roomToken":"…"}
```

Server replies `{"type":"joined","peerId":"…","peers":["…"]}`; notifies existing members with `{"type":"peer-joined","peerId":"…"}`. Same authenticated device cannot have two simultaneous memberships. Unknown/unauthorized/full rooms return `{"type":"error","code":"…","message":"…"}` and close.

Client sends `{"type":"signal","to":"peer-id","payload":{…}}`; server derives sender from authenticated membership and forwards only within that room as `{"type":"signal","from":"peer-id","payload":{…}}`. Payload supports SDP description, ICE candidate, and track metadata as SDK internal types. Server never trusts caller-supplied `from` or changes sender identity.

Client `{"type":"ping"}` → server `{"type":"pong"}`. Socket close/timeout → `{"type":"peer-left","peerId":"…"}` to remaining room members. Session teardown closes the socket. Bound message sizes, queues, rooms and device storage; disconnect slow consumers rather than queue without limit.

## ICE configuration

No public STUN service is silently selected. `STUN_URLS` and `TURN_URLS` are configured by deployment. When TURN is configured, require `TURN_SHARED_SECRET` and return short-lived HMAC credentials; never send the shared secret to clients. Host candidates allow loopback/LAN laboratory verification without TURN, but are not evidence of Internet reachability.

## SDK public API for the first slice

```ts
type SourceKind = "camera" | "screen" | "microphone";
type MediaState = "idle" | "joining" | "joined" | "leaving" | "destroyed";
// @parentview/media-sdk
// new MediaClient({ signaling, peers })
// client.state$: Observable<MediaState>
// client.peers$: Observable<readonly string[]>
// client.tracks$: Observable<readonly RemoteTrack[]>
// client.errors$: Observable<Error>
// client.messages$: Observable<{ peerId: string; data: string }>
// client.join({ roomId, roomToken, deviceToken }): Promise<void>
// client.publish({ id, kind, track }): Promise<void> — client owns track after success
// client.unpublish(id): Promise<void>
// client.send(data: string): void
// client.leave(): Promise<void> — reusable
// client.destroy(): Promise<void> — terminal, idempotent
// Browser composition entry: createBrowserMediaClient({ signalingUrl, iceServers })
```

`track`/remote playback handle are browser-specific at the browser entry; core uses an opaque media-track port, never imports DOM globals. Browser adaptation helpers convert native tracks. Adapters must provide proper ordered per-peer negotiation, early-ICE buffering and deterministic disposal. Data channel messages are application data, never OS input commands.
