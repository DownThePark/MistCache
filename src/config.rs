use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;
use log::warn;

#[derive(Deserialize, Serialize, Clone)]
pub struct Config {
    pub http_bind: String,
    pub dns_bind: String,
    pub cache_dir: String,
    /// Maximum cache size in GB. Least recently used entries are purged once
    /// it is exceeded. 0 means unlimited.
    #[serde(default)]
    pub max_cache_size_gb: u64,
    pub dns_enabled: bool,
    pub intercept_ip: Ipv4Addr,
    pub upstream_dns: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind: "0.0.0.0:80".to_string(),
            dns_bind: "0.0.0.0:53".to_string(),
            cache_dir: "./cache".to_string(),
            max_cache_size_gb: 0,
            dns_enabled: true,
            intercept_ip: Ipv4Addr::new(127, 0, 0, 1),
            upstream_dns: "1.1.1.1".to_string(),
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
