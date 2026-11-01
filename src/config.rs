use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use log::warn;

#[derive(Deserialize, Serialize, Clone)]
pub struct Config {
    pub http_bind: SocketAddr,
    pub dns_bind: SocketAddr,
    pub cache_dir: PathBuf,
    /// Maximum cache size in GB. Least recently used entries are purged once
    /// it is exceeded. 0 means unlimited.
    #[serde(default)]
    pub max_cache_size_gb: u64,
    pub dns_enabled: bool,
    pub intercept_ip: Ipv4Addr,
    pub upstream_dns: IpAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind: SocketAddr::from(([0, 0, 0, 0], 80)),
            dns_bind: SocketAddr::from(([0, 0, 0, 0], 53)),
            cache_dir: PathBuf::from("./cache"),
            max_cache_size_gb: 0,
            dns_enabled: true,
            intercept_ip: Ipv4Addr::new(127, 0, 0, 1),
            upstream_dns: IpAddr::from([1, 1, 1, 1]),
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
