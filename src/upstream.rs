use log::{info, warn};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

// The real Steam server that cache misses are fetched from
pub const HOST: &str = "lancache.steamcontent.com";
const PORT: u16 = 80;

// How often to look up the address again, in case it changes
const REFRESH_INTERVAL: Duration = Duration::from_secs(300);

// Holds the current address of the Steam server. It's looked up at startup
// and refreshed in the background, so requests never wait on DNS.
pub struct Upstream {
    addr: RwLock<SocketAddr>,
}

// Looks up the Steam server's IPv4 address (IPv6 results are skipped)
async fn resolve() -> io::Result<SocketAddr> {
    tokio::net::lookup_host((HOST, PORT))
        .await?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no IPv4 address found for {HOST}")))
}

impl Upstream {
    // Does the first lookup; fails if the host can't be resolved
    pub async fn new() -> io::Result<Arc<Self>> {
        let addr = resolve().await?;
        Ok(Arc::new(Self { addr: RwLock::new(addr) }))
    }

    // Returns the latest known address
    pub fn get(&self) -> SocketAddr {
        *self.addr.read().unwrap_or_else(|e| e.into_inner())
    }

    // Re-checks the address every few minutes in a background task.
    // If a lookup fails, the old address keeps being used.
    pub fn spawn_refresh(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(REFRESH_INTERVAL);
            timer.tick().await; // first tick is immediate, and we just resolved
            loop {
                timer.tick().await;
                match resolve().await {
                    Ok(new) => {
                        // Swap in the new address and log it if it changed
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
