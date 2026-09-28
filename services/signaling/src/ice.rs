//! TURN REST credential generation. Owns no state or transport; never returns the shared secret.
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::{config::Config, models::IceServer};

pub fn ice_servers(config: &Config, peer_id: &str, unix_seconds: u64) -> Vec<IceServer> {
    let mut result = Vec::new();
    if !config.stun_urls.is_empty() {
        result.push(IceServer {
            urls: config.stun_urls.clone(),
            username: None,
            credential: None,
        });
    }
    if !config.turn_urls.is_empty() {
        // HMAC-SHA1 is coturn's TURN REST API algorithm, not a password hash.
        let username = format!("{}:{peer_id}", unix_seconds + config.turn_ttl.as_secs());
        let secret = config
            .turn_shared_secret
            .as_ref()
            .expect("configuration validated at startup");
        let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts arbitrary key size");
        mac.update(username.as_bytes());
        let credential = STANDARD.encode(mac.finalize().into_bytes());
        result.push(IceServer {
            urls: config.turn_urls.clone(),
            username: Some(username),
            credential: Some(credential),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn turn_credentials_are_expiring_hmac_and_never_the_secret() {
        let config = Config {
            turn_urls: vec!["turn:relay.example.test:3478".into()],
            turn_shared_secret: Some("test-only-secret".into()),
            turn_ttl: Duration::from_secs(600),
            ..Config::default()
        };
        let servers = ice_servers(&config, "peer-123", 1000);
        assert_eq!(servers[0].username.as_deref(), Some("1600:peer-123"));
        let credential = STANDARD
            .decode(servers[0].credential.as_ref().unwrap())
            .unwrap();
        let mut mac = Hmac::<Sha1>::new_from_slice(b"test-only-secret").unwrap();
        mac.update(b"1600:peer-123");
        mac.verify_slice(&credential).unwrap();
        assert!(!serde_json::to_string(&servers)
            .unwrap()
            .contains("test-only-secret"));
        assert_ne!(servers, ice_servers(&config, "peer-123", 1001));
    }

    #[test]
    fn empty_configuration_does_not_choose_public_servers() {
        assert!(ice_servers(&Config::default(), "peer", 0).is_empty());
    }

    #[test]
    fn turn_configuration_requires_secret() {
        let config = Config {
            turn_urls: vec!["turn:relay.example.test".into()],
            ..Config::default()
        };
        assert!(config
            .validate()
            .unwrap_err()
            .contains("TURN_SHARED_SECRET"));
    }
}
