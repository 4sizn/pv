//! Deployment configuration, validated before serving. No lifecycle or credentials store.
use std::{env, net::SocketAddr, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub allowed_origins: Vec<String>,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub turn_shared_secret: Option<String>,
    pub turn_ttl: Duration,
    pub device_ttl: Duration,
    pub room_ttl: Duration,
    pub join_timeout: Duration,
    pub heartbeat_timeout: Duration,
    pub write_timeout: Duration,
    pub cleanup_interval: Duration,
    pub max_devices: usize,
    pub max_rooms: usize,
    pub max_connections: usize,
    pub queue_capacity: usize,
    pub max_message_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".parse().expect("literal address"),
            allowed_origins: vec![
                "http://localhost:1420".into(),
                "http://127.0.0.1:1420".into(),
                "tauri://localhost".into(),
                "http://tauri.localhost".into(),
            ],
            stun_urls: vec![],
            turn_urls: vec![],
            turn_shared_secret: None,
            turn_ttl: Duration::from_secs(3600),
            device_ttl: Duration::from_secs(7 * 24 * 3600),
            room_ttl: Duration::from_secs(24 * 3600),
            join_timeout: Duration::from_secs(10),
            heartbeat_timeout: Duration::from_secs(45),
            write_timeout: Duration::from_secs(5),
            cleanup_interval: Duration::from_secs(15),
            max_devices: 10_000,
            max_rooms: 2_000,
            max_connections: 1_024,
            queue_capacity: 64,
            max_message_bytes: 65_536,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let defaults = Self::default();
        let result = Self {
            bind: env::var("SIGNALING_BIND")
                .unwrap_or_else(|_| defaults.bind.to_string())
                .parse()
                .map_err(|_| "SIGNALING_BIND must be a socket address")?,
            allowed_origins: list("ALLOWED_ORIGINS", defaults.allowed_origins),
            stun_urls: list("STUN_URLS", vec![]),
            turn_urls: list("TURN_URLS", vec![]),
            turn_shared_secret: env::var("TURN_SHARED_SECRET")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            turn_ttl: seconds("TURN_CREDENTIAL_TTL_SECONDS", defaults.turn_ttl)?,
            device_ttl: seconds("DEVICE_TTL_SECONDS", defaults.device_ttl)?,
            room_ttl: seconds("ROOM_TTL_SECONDS", defaults.room_ttl)?,
            join_timeout: seconds("JOIN_TIMEOUT_SECONDS", defaults.join_timeout)?,
            heartbeat_timeout: seconds("HEARTBEAT_TIMEOUT_SECONDS", defaults.heartbeat_timeout)?,
            max_devices: number("MAX_DEVICES", defaults.max_devices)?,
            max_rooms: number("MAX_ROOMS", defaults.max_rooms)?,
            max_connections: number("MAX_CONNECTIONS", defaults.max_connections)?,
            queue_capacity: number("SIGNAL_QUEUE_CAPACITY", defaults.queue_capacity)?,
            max_message_bytes: number("MAX_MESSAGE_BYTES", defaults.max_message_bytes)?,
            ..defaults
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.turn_urls.is_empty()
            && self
                .turn_shared_secret
                .as_deref()
                .is_none_or(|secret| secret.trim().is_empty())
        {
            return Err("TURN_SHARED_SECRET is required when TURN_URLS is configured".into());
        }
        if self.allowed_origins.is_empty()
            || self
                .allowed_origins
                .iter()
                .any(|origin| !valid_origin(origin))
        {
            return Err(
                "ALLOWED_ORIGINS must contain exact http(s) origins or tauri://localhost without wildcards".into(),
            );
        }
        if self
            .stun_urls
            .iter()
            .any(|url| !url.starts_with("stun:") && !url.starts_with("stuns:"))
            || self
                .turn_urls
                .iter()
                .any(|url| !url.starts_with("turn:") && !url.starts_with("turns:"))
        {
            return Err("STUN_URLS and TURN_URLS must use their respective URI schemes".into());
        }
        if [
            self.max_devices,
            self.max_rooms,
            self.max_connections,
            self.queue_capacity,
            self.max_message_bytes,
        ]
        .contains(&0)
            || [
                self.turn_ttl,
                self.device_ttl,
                self.room_ttl,
                self.join_timeout,
                self.heartbeat_timeout,
                self.write_timeout,
                self.cleanup_interval,
            ]
            .contains(&Duration::ZERO)
        {
            return Err("Capacity and duration limits must be positive".into());
        }
        Ok(())
    }
}

fn valid_origin(origin: &str) -> bool {
    // WKWebView uses this exact local origin for packaged Tauri content. Other
    // custom schemes/hosts are not permitted, even when supplied in configuration.
    if origin == "tauri://localhost" {
        return true;
    }
    !origin.contains('*')
        && origin.parse::<axum::http::Uri>().ok().is_some_and(|uri| {
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri
                    .authority()
                    .is_some_and(|authority| !authority.as_str().contains('@'))
                && uri.path_and_query().is_none_or(|path| path.as_str() == "/")
        })
}

fn list(name: &str, default: Vec<String>) -> Vec<String> {
    env::var(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or(default)
}

fn number(name: &str, default: usize) -> Result<usize, String> {
    env::var(name)
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("{name} must be a positive integer"))
        })
        .unwrap_or(Ok(default))
}

fn seconds(name: &str, default: Duration) -> Result<Duration, String> {
    number(name, default.as_secs() as usize).map(|value| Duration::from_secs(value as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_must_be_exact_http_or_known_tauri_origins() {
        for origin in [
            "*",
            "https://*.example.test",
            "file:///test",
            "https://example.test/path",
            "https://example.test?query=1",
            "https://user@example.test",
            "tauri://evil.example",
            "tauri://localhost:1420",
            "tauri://localhost/",
            "tauri://localhost/path",
            "custom://localhost",
        ] {
            let config = Config {
                allowed_origins: vec![origin.into()],
                ..Config::default()
            };
            assert!(config.validate().is_err(), "origin {origin} was accepted");
        }
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn zero_limits_and_wrong_ice_schemes_are_rejected() {
        assert!(Config {
            queue_capacity: 0,
            ..Config::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            room_ttl: Duration::ZERO,
            ..Config::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            stun_urls: vec!["https://example.test".into()],
            ..Config::default()
        }
        .validate()
        .is_err());
    }
}
