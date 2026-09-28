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

## Native media protocol

The native engine reserves camera/video, screen/video and microphone/audio slots once per peer. Only the smaller authenticated peer ID creates the offer and ordered data channel. The answerer adopts the offered transceivers; it does not create competing slots. A native description includes exactly three distinct kind/MID bindings:

```json
{"type":"description","description":{"type":"offer","sdp":"…","slots":[{"kind":"camera","mid":"0"},{"kind":"screen","mid":"1"},{"kind":"microphone","mid":"2"}]}}
```

The MID strings above are examples, not assigned constants. The adapter validates the bindings against actual negotiated transceivers and checks the answer preserves them. Publishing and removing sources attaches/detaches existing senders without a new SDP exchange. Slot count and kind are fixed for the peer lifetime; source identity may change.

After negotiation and every publication change, the runtime sends the full active manifest for that peer:

```json
{"type":"sources","revision":3,"sources":[{"id":"camera-2","kind":"camera","mid":"0"}]}
```

An empty `sources` array clears remote publications. There are at most three entries, with distinct IDs, kinds and MIDs. IDs/MIDs are nonempty, at most 128 UTF-8 bytes and contain no control characters. Every kind/MID must match the negotiated slots. The runtime ignores older revisions; malformed current manifests close that peer and emit an error. Receiver tracks stay alive across unpublish/re-publish. A manifest identifies a publication, but cannot mark the exact boundary between already queued RTP frames and a new source.

The Rust `NativeMediaClient` owns `publish(id, source)`, `unpublish(id)` and `media_stats()` alongside data/session commands. Publishing consumes the source even if rejected; deterministic source cleanup belongs to the runtime and the injected source's close/Drop contracts. Stats expose scalar diagnostic observations, never decoded buffers, and currently stay in the Rust API. The TypeScript facade exposes readonly `localSources$` and `remoteSources$` from authoritative snapshots; it has no capture command or raw-frame IPC API.

The signaling server routes these payloads only between authenticated members and remains media-agnostic. Browser publication negotiation uses its own internal track protocol. Mixed browser/native media interoperability is not supported by this fixed-slot native protocol.
