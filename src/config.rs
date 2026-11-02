use serde::Deserialize;
use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use log::warn;

// Commented template written to disk when no config.toml exists yet.
const DEFAULT_CONFIG: &str = include_str!("../config.toml");

// Ports are fixed because clients expect the cache on the standard ports.
pub const HTTP_PORT: u16 = 80;
pub const DNS_PORT: u16 = 53;

// Settings read from config.toml.
#[derive(Deserialize, Clone)]
pub struct Config {
    // Address the HTTP server listens on (port 80)
    pub http_bind: Ipv4Addr,
    // Address the DNS server listens on (port 53)
    pub dns_bind: Ipv4Addr,
    // Where cached files are stored
    pub cache_dir: PathBuf,
    // Size limit in GB before old entries are purged (0 = unlimited)
    #[serde(default)]
    pub max_cache_size_gb: u64,
    // Run the built-in DNS server, which answers with http_bind
    pub dns_enabled: bool,
    // DNS server that other lookups are forwarded to (port 53)
    pub upstream_dns: Ipv4Addr,
}

impl Config {
    pub fn http_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(self.http_bind, HTTP_PORT)
    }

    pub fn dns_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(self.dns_bind, DNS_PORT)
    }

    pub fn upstream_dns_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(self.upstream_dns, DNS_PORT)
    }

    // DNS tells clients to connect to http_bind, so it can't be 0.0.0.0
    // while DNS is enabled.
    pub fn validate(&self) -> Result<(), String> {
        if self.dns_enabled && self.http_bind.is_unspecified() {
            return Err(format!(
                "http_bind is {} but dns_enabled is true: DNS answers use http_bind as the \
                 address clients connect to, so set http_bind to this machine's LAN IP",
                self.http_bind
            ));
        }
        Ok(())
    }
}

// Reads config.toml, creating it from the template on first run.
pub fn load_or_create_config() -> Config {
    let config_path = "config.toml";
    if !Path::new(config_path).exists() {
        fs::write(config_path, DEFAULT_CONFIG).expect("Failed to create config.toml");
        warn!("Created default config.toml. Edit it and restart.");
    }
    let config_str = fs::read_to_string(config_path).expect("Could not read config.toml");
    toml::from_str(&config_str).expect("Error parsing config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shipped template must parse and pass validation as-is.
    #[test]
    fn default_config_is_valid() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(cfg.validate().is_ok());
    }
}
