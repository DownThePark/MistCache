use log::{info, error};
use std::net::TcpListener;
use std::sync::Arc;
use pingora::prelude::*;
use pingora::proxy::HttpProxy;
use crate::config::Config;
use crate::eviction::CacheIndex;
use crate::proxy::LANCache;

pub fn create_http_service(engine: &Server, cfg: &Config) -> Option<pingora::services::listening::Service<HttpProxy<LANCache>>> {
    match TcpListener::bind(cfg.http_bind) {
        Ok(_) => {
            info!("SUCCESS: LANCache HTTP listening on {}", cfg.http_bind);
        }
        Err(e) => {
            error!("FAILED: Could not bind to {}: {}", cfg.http_bind, e);
            error!("Ensure you have permission (sudo) and the address is available.");
            return None;
        }
    }

    let index = Arc::new(CacheIndex::load(&cfg.cache_dir, cfg.max_cache_size_gb));

    let mut http_service = pingora::proxy::http_proxy_service(
        &engine.configuration,
        LANCache { config: cfg.clone(), index }
    );
    http_service.add_tcp(&cfg.http_bind.to_string());

    Some(http_service)
}
