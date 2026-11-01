use log::{info, error};
use std::net::{TcpListener, SocketAddr};
use std::sync::Arc;
use pingora::prelude::*;
use pingora::proxy::HttpProxy;
use crate::config::Config;
use crate::eviction::CacheIndex;
use crate::proxy::LANCache;

pub fn create_http_service(engine: &Server, cfg: &Config) -> Option<pingora::services::listening::Service<HttpProxy<LANCache>>> {
    let bind_addr: SocketAddr = cfg.http_bind.parse().expect("Invalid http_bind format in config");

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

    let index = Arc::new(CacheIndex::load(&cfg.cache_dir, cfg.max_cache_size_gb));

    let mut http_service = pingora::proxy::http_proxy_service(
        &engine.configuration,
        LANCache { config: cfg.clone(), index }
    );
    http_service.add_tcp(&cfg.http_bind);
    
    Some(http_service)
}
