# TURN infrastructure status

The Compose file is a local container template. Port publication is restricted to the host loopback interface. Its configuration parses successfully, but no Docker daemon was available during bootstrap verification, so container startup and relay connectivity are unverified.

The default bridge-network container can advertise an internal relay IP that a host browser cannot reach, especially across a macOS Docker VM. This template intentionally does not guess an `external-ip`: the correct value depends on the actual network topology. Starting the container or receiving temporary TURN credentials does not prove relay operation.

Before enabling `TURN_URLS` on the signaling server:

1. Choose the test/deployment topology and reachable TURN listener address. Current loopback-only published ports do not support remote devices.
2. Configure coturn's advertised relay address with the appropriate `external-ip` mapping when behind NAT. Relay ports 49160–49200 must map to the same externally advertised ports, and both the clients and peers must reach that address.
3. Keep `TURN_SHARED_SECRET` identical in signaling and coturn; never place it in client configuration. Signaling issues short-lived credentials instead.
4. Use a dedicated test with `iceTransportPolicy: "relay"`, then assert the selected ICE candidate pair uses relay candidates and actual video/audio/data passes in both directions. The current laboratory/E2E does not force relay and is not this test.
5. For a public deployment, configure TLS/certificates, network/firewall rules and an explicit policy denying private-network peer destinations. The local template does not provide those deployment controls.

The required `external-ip` behavior and NAT port-preservation requirement are documented in the [official coturn configuration example](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf). Container topology and forced-relay proof remain a separate acceptance milestone.
