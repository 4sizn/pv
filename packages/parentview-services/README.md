# ParentView services

The only package that names product roles: `parent` is the supported device owner and `child` is a helper. Both can initiate a support request; media publishing remains a feature of the role-neutral SDK. `ParentViewPolicy` currently permits a child to control a different parent's device, subject to all consent gates.

`SupportSessionService` is a local receiving-side policy foundation, not a distributed approval/pairing implementation. Composition must supply participant identities from authenticated pairing records. Never populate roles or control grants directly from untrusted DataChannel JSON. The sender passed to `receiveInput` must be the transport-authenticated peer, and an OS adapter must validate platform permission on every input execution.

## Module contracts

| Module | Owns / responsibility | Must not own | Contract and teardown |
| --- | --- | --- | --- |
| SupportSessionService | Current product session, consent, exclusive input grant, serialized input and cleanup | WebRTC SDP/ICE, sockets, concrete OS | Inject narrow media/input ports. state$ is an immutable snapshot; errors$ carries cleanup/join failures. end revokes authority before asynchronous cleanup; failed cleanup blocks new requests and is retriable. destroy is terminal to commands and completes streams only after cleanup succeeds. |
| ParentViewPolicy | Product role eligibility | Device authentication, media capability | Pure eligibility; no owned resources |
| NetworkService | Environment availability snapshot and listener | Peer connection/retry state | Inject environment port; destroy unsubscribes and completes snapshot stream |
| DeviceService | Latest device catalog and refresh generation | Capture tracks, family devices, product roles | Inject catalog port; stale completion ignored, destroy unsubscribes and completes streams |
| types | Immutable role, participant, state and narrow port contracts | Implementations | Imports only SDK core types and Observable |

Session approval, screen consent and control grant are independent. OS capture permission success must precede `confirmScreenConsent`. Receiver-side revocation invalidates already queued input using a grant generation. The media client itself remains owned by its composition root; ending a support session calls `leave`, not `destroy` on shared application resources.

Run `bun test packages/parentview-services` from the root. Tests cover unauthorized senders, exclusive control, permission loss, queued input after revocation, caller mutation, synchronous Rx reentrancy, late asynchronous completion, retryable cleanup failure and service subscription teardown.
