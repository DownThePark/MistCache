use log::{info, error};
use std::net::TcpListener;
use std::sync::Arc;
use pingora::prelude::*;
use pingora::proxy::HttpProxy;
use crate::config::Config;
use crate::eviction::CacheIndex;
use crate::proxy::LANCache;
use crate::upstream::Upstream;

// Builds the HTTP caching proxy, or returns None if the port can't be used.
pub fn create_http_service(engine: &Server, cfg: &Config, upstream: Arc<Upstream>) -> Option<pingora::services::listening::Service<HttpProxy<LANCache>>> {
    let bind_addr = cfg.http_addr();
    // Test-bind first to give a clear error (port 80 usually needs sudo)
    match TcpListener::bind(bind_addr) {
        Ok(_) => {
            info!("SUCCESS: LANCache HTTP listening on {}", bind_addr);
        }
        Err(e) => {
            error!("FAILED: Could not bind to {}: {}", bind_addr, e);
            error!("Ensure you have permission (sudo) and the address is available.");
            return None;
        }
    }

    // Scan the cache folder so size tracking and eviction know what's stored
    let index = Arc::new(CacheIndex::load(&cfg.cache_dir, cfg.max_cache_size_gb));

    let mut http_service = pingora::proxy::http_proxy_service(
        &engine.configuration,
        LANCache { config: cfg.clone(), index, upstream }
    );
    http_service.add_tcp(&bind_addr.to_string());

    Some(http_service)
}
