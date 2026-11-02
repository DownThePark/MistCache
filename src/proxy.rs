use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use log::{info, warn};
use pingora::http::ResponseHeader;
use pingora::prelude::*;
use pingora::proxy::{ProxyHttp, Session};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use crate::config::Config;
use crate::eviction::CacheIndex;
use crate::upstream::{self, Upstream};

// How much of a cached file is read at a time when sending it to a client
const CHUNK: usize = 64 * 1024;

// State for one cache miss while its download is written to disk
#[derive(Default)]
pub struct CacheCtx {
    file_handle: Option<File>,
    temp_path: Option<PathBuf>,
    final_path: Option<PathBuf>,
    bytes_written: u64,
}

impl Drop for CacheCtx {
    // Deletes the half-written temp file if the download didn't finish
    // (client or server disconnected, disk write failed, etc.)
    fn drop(&mut self) {
        self.file_handle = None;
        if let Some(tmp) = self.temp_path.take() {
            let _ = fs::remove_file(tmp);
        }
    }
}

// The caching proxy. Pingora calls the methods below for each request.
pub struct MistCache {
    pub config: Config,
    pub index: Arc<CacheIndex>,
    pub upstream: Arc<Upstream>,
}

// Makes a unique temp file name per request, so two downloads of the same
// file never write into the same file
fn temp_path_for(final_path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.temp", NEXT.fetch_add(1, Ordering::Relaxed)));
    final_path.with_file_name(name)
}

// Creates the temp file (and any missing folders) for a download
async fn open_temp_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(tokio::fs::File::create(path).await?.into_std().await)
}

// Sends a cached file to the client as a full 200 response
async fn serve_file(session: &mut Session, mut file: tokio::fs::File, len: u64) -> Result<()> {
    let mut header = ResponseHeader::build(200, Some(3))?;
    header.insert_header("Content-Type", "application/octet-stream")?;
    header.insert_header("Content-Length", len)?;
    header.insert_header("X-Cache", "HIT")?;
    session.write_response_header(Box::new(header), false).await?;

    // Stream the file in chunks so large files don't fill memory
    let mut buf = BytesMut::with_capacity(CHUNK);
    loop {
        buf.reserve(CHUNK);
        let n = file
            .read_buf(&mut buf)
            .await
            .map_err(|e| Error::because(ErrorType::ReadError, "reading cache file", e))?;
        if n == 0 {
            break;
        }
        session.write_response_body(Some(buf.split().freeze()), false).await?;
    }
    session.write_response_body(None, true).await
}

#[async_trait]
impl ProxyHttp for MistCache {
    type CTX = CacheCtx;
    fn new_ctx(&self) -> Self::CTX {
        CacheCtx::default()
    }

    // Picks the server to fetch from on a cache miss
    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        // Uses the already-resolved address, so no DNS lookup happens here
        Ok(Box::new(HttpPeer::new(
            self.upstream.get(),
            false,
            upstream::HOST.to_string(),
        )))
    }

    // Runs first for every request: serves cache hits, prepares cache misses
    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let req = session.req_header();
        // Only GET requests are cached; anything else passes straight through
        if req.method != "GET" {
            return Ok(false);
        }
        let uri_path = req.uri.path().to_owned();

        // The URL path becomes a file path, so reject anything unsafe
        // (like ".." escaping the cache folder) or empty
        let rel = Path::new(uri_path.trim_start_matches('/'));
        if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            session.respond_error(400).await?;
            return Ok(true);
        }
        let final_path = self.config.cache_dir.join(rel);

        // Client address, used only in log messages
        let client_ip = session.client_addr()
            .and_then(|addr| addr.as_inet())
            .map(|inet| inet.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Cache hit: the file already exists on disk
        if let Ok(metadata) = tokio::fs::metadata(&final_path).await {
            if metadata.is_file() {
                // If the file was purged just now, the open fails and this
                // is treated as a miss
                if let Ok(file) = tokio::fs::File::open(&final_path).await {
                    info!("[{}] CACHE HIT: {} ({} KB)", client_ip, uri_path, metadata.len() / 1024);
                    // Mark as recently used so it's evicted last
                    self.index.touch(&final_path);
                    serve_file(session, file, metadata.len()).await?;
                    return Ok(true);
                }
            }
        }

        // Cache miss: remember where to save it. The temp file is only
        // created once the server replies 200, so errors are never cached.
        info!("[{}] CACHE MISS: {}", client_ip, uri_path);
        ctx.temp_path = Some(temp_path_for(&final_path));
        ctx.final_path = Some(final_path);
        Ok(false)
    }

    // Runs when the upstream response headers arrive
    async fn response_filter(&self, _session: &mut Session, upstream_response: &mut ResponseHeader, ctx: &mut Self::CTX) -> Result<()> {
        let Some(tmp) = ctx.temp_path.clone() else { return Ok(()) };

        // Only successful (200) responses are cached
        if upstream_response.status.as_u16() != 200 {
            ctx.temp_path = None;
            ctx.final_path = None;
            return Ok(());
        }

        // Open the temp file to start saving the download
        match open_temp_file(&tmp).await {
            Ok(file) => ctx.file_handle = Some(file),
            Err(e) => {
                warn!("Not caching {}: {}", tmp.display(), e);
                ctx.temp_path = None;
                ctx.final_path = None;
            }
        }
        Ok(())
    }

    // Runs for each chunk of the response: saves it to disk while it passes
    // through to the client
    fn response_body_filter(&self, _session: &mut Session, body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut Self::CTX) -> Result<Option<Duration>> {
        if let (Some(chunk), Some(file)) = (body.as_ref(), ctx.file_handle.as_mut()) {
            match file.write_all(chunk) {
                Ok(()) => ctx.bytes_written += chunk.len() as u64,
                Err(e) => {
                    // Give up on caching this file rather than keep incomplete
                    // data. The temp file is deleted when the context drops.
                    warn!("Cache write failed, entry will not be stored: {}", e);
                    ctx.file_handle = None;
                }
            }
        }

        // Download complete: move the temp file into place and track its size
        if end_of_stream {
            if let Some(mut file) = ctx.file_handle.take() {
                if file.flush().is_ok() {
                    if let (Some(tmp), Some(dest)) = (ctx.temp_path.take(), ctx.final_path.take()) {
                        if fs::rename(&tmp, &dest).is_ok() {
                            self.index.record(dest, ctx.bytes_written);
                        } else {
                            let _ = fs::remove_file(&tmp);
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}
