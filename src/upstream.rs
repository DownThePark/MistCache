use log::{info, warn};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The real Steam content host that cache misses are fetched from.
pub const HOST: &str = "lancache.steamcontent.com";
const PORT: u16 = 80;

/// How often the address is looked up again, in case the host moves.
const REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// The current address of the upstream host. It is resolved once at startup
/// and refreshed in the background, so requests never wait on DNS.
pub struct Upstream {
    addr: RwLock<SocketAddr>,
}

/// Resolves the upstream host to an IPv4 address; IPv6 results are ignored.
async fn resolve() -> io::Result<SocketAddr> {
    tokio::net::lookup_host((HOST, PORT))
        .await?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no IPv4 address found for {HOST}")))
}

impl Upstream {
    /// Resolves the host for the first time. Fails if it cannot be resolved.
    pub async fn new() -> io::Result<Arc<Self>> {
        let addr = resolve().await?;
        Ok(Arc::new(Self { addr: RwLock::new(addr) }))
    }

    /// The most recently resolved address.
    pub fn get(&self) -> SocketAddr {
        *self.addr.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a background task that re-resolves the host on a timer. If a
    /// lookup fails, the previous address stays in use.
    pub fn spawn_refresh(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(REFRESH_INTERVAL);
            timer.tick().await; // the first tick is immediate; we just resolved
            loop {
                timer.tick().await;
                match resolve().await {
                    Ok(new) => {
                        let old = std::mem::replace(
                            &mut *this.addr.write().unwrap_or_else(|e| e.into_inner()),
                            new,
                        );
                        if old != new {
                            info!("Upstream {} changed: {} -> {}", HOST, old.ip(), new.ip());
                        }
                    }
                    Err(e) => warn!("Could not refresh upstream {}: {} (keeping {})", HOST, e, this.get().ip()),
                }
            }
        });
    }
}
