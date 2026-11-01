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

/// Read size used when streaming a cached file to the client.
const CHUNK: usize = 64 * 1024;

/// Cached files smaller than this are treated as leftovers of an old error
/// response and are deleted instead of being served.
const MIN_VALID_SIZE: u64 = 200;

/// Per-request state for a cache miss that is being written to disk.
#[derive(Default)]
pub struct CacheCtx {
    file_handle: Option<File>,
    temp_path: Option<PathBuf>,
    final_path: Option<PathBuf>,
    bytes_written: u64,
}

impl Drop for CacheCtx {
    /// Removes the partial download if the request ended before it was
    /// finalized (client or upstream aborted, disk write failed, ...).
    fn drop(&mut self) {
        self.file_handle = None;
        if let Some(tmp) = self.temp_path.take() {
            let _ = fs::remove_file(tmp);
        }
    }
}

pub struct LANCache {
    pub config: Config,
    pub index: Arc<CacheIndex>,
}

/// Returns a temp file name unique to this request, so that concurrent misses
/// for the same path never write into the same file.
fn temp_path_for(final_path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.temp", NEXT.fetch_add(1, Ordering::Relaxed)));
    final_path.with_file_name(name)
}

async fn open_temp_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(tokio::fs::File::create(path).await?.into_std().await)
}

/// Streams a cached file to the client as a complete 200 response.
async fn serve_file(session: &mut Session, mut file: tokio::fs::File, len: u64) -> Result<()> {
    let mut header = ResponseHeader::build(200, Some(3))?;
    header.insert_header("Content-Type", "application/octet-stream")?;
    header.insert_header("Content-Length", len)?;
    header.insert_header("X-Cache", "HIT")?;
    session.write_response_header(Box::new(header), false).await?;

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
impl ProxyHttp for LANCache {
    type CTX = CacheCtx;
    fn new_ctx(&self) -> Self::CTX {
        CacheCtx::default()
    }

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        Ok(Box::new(HttpPeer::new(
            ("lancache.steamcontent.com", 80),
            false,
            "lancache.steamcontent.com".to_string(),
        )))
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let req = session.req_header();
        // Only plain GETs are cached; anything else is proxied untouched.
        if req.method != "GET" {
            return Ok(false);
        }
        let uri_path = req.uri.path().to_owned();

        // The URI path becomes a file path, so refuse anything that could
        // escape the cache directory (`..`) or that names no file at all.
        let rel = Path::new(uri_path.trim_start_matches('/'));
        if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            session.respond_error(400).await?;
            return Ok(true);
        }
        let final_path = self.config.cache_dir.join(rel);

        let client_ip = session.client_addr()
            .and_then(|addr| addr.as_inet())
            .map(|inet| inet.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        if let Ok(metadata) = tokio::fs::metadata(&final_path).await {
            if metadata.is_file() {
                if metadata.len() < MIN_VALID_SIZE {
                    // Delete corrupted file and treat the request as a miss
                    let _ = tokio::fs::remove_file(&final_path).await;
                    self.index.forget(&final_path);
                    info!("Deleted corrupted cache entry: {}", uri_path);
                } else if let Ok(file) = tokio::fs::File::open(&final_path).await {
                    // If the entry was purged between the check and the open, the
                    // open fails and the request falls through to a miss.
                    info!("[{}] CACHE HIT: {} ({} KB)", client_ip, uri_path, metadata.len() / 1024);
                    self.index.touch(&final_path);
                    serve_file(session, file, metadata.len()).await?;
                    return Ok(true);
                }
            }
        }

        // The temp file is only created once the upstream answers 200 (see
        // `response_filter`), so errors and partial responses are never cached.
        info!("[{}] CACHE MISS: {}", client_ip, uri_path);
        ctx.temp_path = Some(temp_path_for(&final_path));
        ctx.final_path = Some(final_path);
        Ok(false)
    }

    async fn response_filter(&self, _session: &mut Session, upstream_response: &mut ResponseHeader, ctx: &mut Self::CTX) -> Result<()> {
        let Some(tmp) = ctx.temp_path.clone() else { return Ok(()) };

        if upstream_response.status.as_u16() != 200 {
            ctx.temp_path = None;
            ctx.final_path = None;
            return Ok(());
        }

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

    fn response_body_filter(&self, _session: &mut Session, body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut Self::CTX) -> Result<Option<Duration>> {
        if let (Some(chunk), Some(file)) = (body.as_ref(), ctx.file_handle.as_mut()) {
            match file.write_all(chunk) {
                Ok(()) => ctx.bytes_written += chunk.len() as u64,
                Err(e) => {
                    // Never finalize a file with missing data; the partial
                    // temp file is removed when the request context drops.
                    warn!("Cache write failed, entry will not be stored: {}", e);
                    ctx.file_handle = None;
                }
            }
        }

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
