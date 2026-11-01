use async_trait::async_trait;
use bytes::Bytes;
use log::info;
use pingora::http::ResponseHeader;
use pingora::prelude::*;
use pingora::proxy::{ProxyHttp, Session};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use crate::config::Config;
use crate::eviction::CacheIndex;

pub struct CacheCtx {
    file_handle: Option<File>,
    temp_path: Option<PathBuf>,
    final_path: Option<PathBuf>,
    pub uri_path: String,
    pub cache_dir: String,
}

pub struct LANCache {
    pub config: Config,
    pub index: Arc<CacheIndex>,
}

#[async_trait]
impl ProxyHttp for LANCache {
    type CTX = CacheCtx;
    fn new_ctx(&self) -> Self::CTX {
        CacheCtx {
            file_handle: None,
            temp_path: None,
            final_path: None,
            uri_path: String::new(),
            cache_dir: self.config.cache_dir.clone(),
        }
    }

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        Ok(Box::new(HttpPeer::new(
            ("lancache.steamcontent.com", 80),
            false,
            "lancache.steamcontent.com".to_string(),
        )))
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let path = session.req_header().uri.path();
        ctx.uri_path = path.to_string();
        let base_path = PathBuf::from(&ctx.cache_dir);
        let final_path = base_path.join(path.trim_start_matches('/'));
        let mut temp_path = final_path.clone();
        temp_path.set_extension("temp");

        let client_ip = session.client_addr()
            .and_then(|addr| addr.as_inet())
            .map(|inet| inet.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        if let Ok(metadata) = fs::metadata(&final_path) {
            if metadata.len() < 200 {
                // Delete corrupted file and treat the request as a miss
                let _ = fs::remove_file(&final_path);
                self.index.forget(&final_path);
                info!("Deleted corrupted cache entry: {}", path);
            } else if let Ok(mut file) = tokio::fs::File::open(&final_path).await {
                // If the entry was purged between the check and the open, the
                // open fails and the request falls through to a miss.
                info!("[{}] CACHE HIT: {} ({} KB)", client_ip, path, metadata.len() / 1024);
                self.index.touch(&final_path);

                let mut header = ResponseHeader::build(200, None).unwrap().to_owned();
                header.insert_header("Content-Type", "application/octet-stream").unwrap();
                header.insert_header("Content-Length", metadata.len()).unwrap();
                header.insert_header("X-Cache", "HIT").unwrap();

                session.write_response_header(Box::new(header), false).await?;
                let mut buf = [0u8; 65536];
                while let Ok(n) = file.read(&mut buf).await {
                    if n == 0 { break; }
                    session.write_response_body(Some(Bytes::copy_from_slice(&buf[..n])), false).await?;
                }
                session.write_response_body(None, true).await?;
                return Ok(true);
            }
        }

        info!("[{}] CACHE MISS: {}", client_ip, path);
        if let Some(parent) = temp_path.parent() { let _ = fs::create_dir_all(parent); }
        let file = File::create(&temp_path).map_err(|_| Error::new(ErrorType::InternalError))?;
        ctx.file_handle = Some(file);
        ctx.temp_path = Some(temp_path);
        ctx.final_path = Some(final_path);
        Ok(false)
    }

    fn response_body_filter(&self, _session: &mut Session, body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut Self::CTX) -> Result<Option<Duration>> {
        if let Some(chunk) = body {
            if let Some(ref mut file) = ctx.file_handle { let _ = file.write_all(chunk); }
        }
        if end_of_stream {
            if let (Some(mut file), Some(tmp), Some(dest)) = (ctx.file_handle.take(), ctx.temp_path.take(), ctx.final_path.take()) {
                let _ = file.flush();
                if fs::rename(&tmp, &dest).is_ok() {
                    if let Ok(meta) = fs::metadata(&dest) {
                        self.index.record(dest, meta.len());
                    }
                }
            }
        }
        Ok(None)
    }
}
