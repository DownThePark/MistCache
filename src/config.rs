use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use log::warn;

/// The HTTP and DNS ports are fixed: clients expect to find the cache on the
/// standard ports, so they cannot be changed in the config.
pub const HTTP_PORT: u16 = 80;
pub const DNS_PORT: u16 = 53;

/// Everything is IPv4-only: listeners, upstream DNS and the upstream host.
#[derive(Deserialize, Serialize, Clone)]
pub struct Config {
    /// IPv4 address the HTTP server listens on (port 80).
    pub http_bind: Ipv4Addr,
    /// IPv4 address the DNS server listens on (port 53).
    pub dns_bind: Ipv4Addr,
    pub cache_dir: PathBuf,
    /// Maximum cache size in GB. Least recently used entries are purged once
    /// it is exceeded. 0 means unlimited.
    #[serde(default)]
    pub max_cache_size_gb: u64,
    /// When enabled, the DNS server answers `lancache.steamcontent.com` with
    /// `http_bind`, so `http_bind` must then be the address clients can reach.
    pub dns_enabled: bool,
    /// IPv4 address of the upstream DNS server (queried on port 53).
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

    /// DNS answers point clients at `http_bind`, so it has to be a real
    /// address; 0.0.0.0 only makes sense when DNS is disabled.
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

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind: Ipv4Addr::LOCALHOST,
            dns_bind: Ipv4Addr::UNSPECIFIED,
            cache_dir: PathBuf::from("./cache"),
            max_cache_size_gb: 0,
            dns_enabled: true,
            upstream_dns: Ipv4Addr::new(1, 1, 1, 1),
        }
    }
}

pub fn load_or_create_config() -> Config {
    let config_path = "config.toml";
    if !Path::new(config_path).exists() {
        let default_config = Config::default();
        let toml_string = toml::to_string_pretty(&default_config).unwrap();
        fs::write(config_path, toml_string).expect("Failed to create config.toml");
        warn!("Created default config.toml. Edit it and restart.");
    }
    let config_str = fs::read_to_string(config_path).expect("Could not read config.toml");
    toml::from_str(&config_str).expect("Error parsing config.toml")
}
