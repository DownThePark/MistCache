mod config;
mod dns;
mod eviction;
mod http;
mod proxy;
mod upstream;

use log::{error, info, warn};
use pingora::prelude::*;
use crate::config::load_or_create_config;
use crate::dns::create_dns_service;
use crate::http::create_http_service;
use crate::upstream::Upstream;

#[tokio::main]
async fn main() {
    // Logger first, so messages from loading the config are not lost.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,pingora_core=warn,pingora_proxy=warn")
    )
    .format_timestamp(None)
    .format_module_path(false)
    .init();

    let cfg = load_or_create_config();
    if let Err(e) = cfg.validate() {
        error!("Invalid config.toml: {}", e);
        std::process::exit(1);
    }

    info!("------------------------------------------");
    info!("Starting LANCache (Rust Engine)");
    info!("Cache Location: {}", cfg.cache_dir.display());
    if cfg.max_cache_size_gb > 0 {
        info!("Cache Limit:    {} GB", cfg.max_cache_size_gb);
    } else {
        info!("Cache Limit:    unlimited");
    }
    info!("HTTP Address:   {}", cfg.http_addr());

    if cfg.dns_enabled {
        info!("DNS Address:    {}", cfg.dns_addr());
        info!("Intercept IP:   {} (http_bind)", cfg.http_bind);
        info!("Upstream DNS:   {}", cfg.upstream_dns_addr());
    }
    info!("------------------------------------------");

    let mut engine = Server::new(None).expect("Failed to initialize engine");

    // Resolve the real Steam host once now and keep it fresh in the background,
    // so cache misses never wait on a DNS lookup.
    let upstream = match Upstream::new().await {
        Ok(u) => u,
        Err(e) => {
            error!("Could not resolve {}: {}", upstream::HOST, e);
            std::process::exit(1);
        }
    };
    let upstream_addr = upstream.get();
    info!("Upstream:       {} -> {}", upstream::HOST, upstream_addr.ip());
    if cfg.dns_enabled && upstream_addr.ip() == cfg.http_bind {
        warn!(
            "{} resolves to the intercept IP ({}): cache misses would loop back to this server. \
             Point this machine's own DNS at a different resolver.",
            upstream::HOST, cfg.http_bind
        );
    }
    upstream.spawn_refresh();

    // Startup failures were already logged; exit non-zero so supervisors notice.
    let Some(http_service) = create_http_service(&engine, &cfg, upstream) else {
        std::process::exit(1);
    };
    engine.add_service(http_service);

    let mut dns_server = if cfg.dns_enabled {
        match create_dns_service(&cfg).await {
            Some(s) => Some(s),
            None => std::process::exit(1),
        }
    } else {
        None
    };

    // Resolves when the DNS server stops; never resolves if DNS is disabled.
    let dns_done = async {
        match dns_server.as_mut() {
            Some(s) => { let _ = s.block_until_done().await; }
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        _ = dns_done => (),
        _ = tokio::task::spawn_blocking(move || engine.run_forever()) => (),
        _ = tokio::signal::ctrl_c() => info!("Shutdown signal received."),
    }
}
