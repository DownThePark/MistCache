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
    // Start the logger first so config errors are visible
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,pingora_core=warn,pingora_proxy=warn")
    )
    .format_timestamp(None)
    .format_module_path(false)
    .init();

    // Load and validate config.toml
    let cfg = load_or_create_config();
    if let Err(e) = cfg.validate() {
        error!("Invalid config.toml: {}", e);
        std::process::exit(1);
    }

    // Startup banner
    info!("------------------------------------------");
    info!("Starting MistCache (Rust Engine)");
    info!("Cache Location: {}", cfg.cache_dir.display());
    if cfg.max_cache_size_gb > 0 {
        info!("Cache Limit:    {} GB", cfg.max_cache_size_gb);
    } else {
        info!("Cache Limit:    unlimited");
    }
    info!("HTTP Address:   {}", cfg.http_bind);

    if cfg.dns_enabled {
        info!("DNS Address:    {}", cfg.dns_bind);
        info!("Upstream DNS:   {}", cfg.upstream_dns);
    }
    info!("------------------------------------------");

    let mut engine = Server::new(None).expect("Failed to initialize engine");

    // Look up the real Steam server once, then keep it fresh in the background
    // so cache misses never wait on DNS
    let upstream = match Upstream::new().await {
        Ok(u) => u,
        Err(e) => {
            error!("Could not resolve {}: {}", upstream::HOST, e);
            std::process::exit(1);
        }
    };
    let upstream_addr = upstream.get();
    info!("Upstream:       {} -> {}", upstream::HOST, upstream_addr.ip());
    // If this machine resolves Steam to itself, cache misses would loop forever
    if cfg.dns_enabled && upstream_addr.ip() == cfg.http_bind {
        warn!(
            "{} resolves to the intercept IP ({}): cache misses would loop back to this server. \
             Point this machine's own DNS at a different resolver.",
            upstream::HOST, cfg.http_bind
        );
    }
    upstream.spawn_refresh();

    // Start the HTTP cache (the error is already logged if it fails)
    let Some(http_service) = create_http_service(&engine, &cfg, upstream) else {
        std::process::exit(1);
    };
    engine.add_service(http_service);

    // Start the DNS server if enabled
    let mut dns_server = if cfg.dns_enabled {
        match create_dns_service(&cfg).await {
            Some(s) => Some(s),
            None => std::process::exit(1),
        }
    } else {
        None
    };

    // Completes when the DNS server stops, or never if DNS is disabled
    let dns_done = async {
        match dns_server.as_mut() {
            Some(s) => { let _ = s.block_until_done().await; }
            None => std::future::pending::<()>().await,
        }
    };

    // Run until DNS stops, the HTTP engine stops, or Ctrl+C is pressed
    tokio::select! {
        _ = dns_done => (),
        _ = tokio::task::spawn_blocking(move || engine.run_forever()) => (),
        _ = tokio::signal::ctrl_c() => info!("Shutdown signal received."),
    }
}
