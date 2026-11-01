use async_trait::async_trait;
use hickory_proto::op::{Header, ResponseCode};
use hickory_proto::rr::{rdata::A, LowerName, Name, RData, Record, RecordType};
use hickory_resolver::config::{ResolverConfig, ResolverOpts, NameServerConfig, Protocol};
use hickory_resolver::error::ResolveErrorKind;
use hickory_resolver::TokioAsyncResolver;
use hickory_server::authority::MessageResponseBuilder;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use log::{info, error, warn};
use std::net::Ipv4Addr;
use crate::config::Config;

/// The only name answered locally; everything else is forwarded upstream.
const INTERCEPT_DOMAIN: &str = "lancache.steamcontent.com.";

#[derive(Clone)]
pub struct LANCacheDns {
    http_bind: Ipv4Addr,
    intercept_name: LowerName,
    resolver: TokioAsyncResolver,
}

pub async fn create_dns_service(cfg: &Config) -> Option<hickory_server::ServerFuture<LANCacheDns>> {
    let mut resolver_config = ResolverConfig::new();
    resolver_config.add_name_server(NameServerConfig::new(cfg.upstream_dns_addr().into(), Protocol::Udp));

    let dns_handler = LANCacheDns {
        // Clients are pointed at the HTTP server's own address.
        http_bind: cfg.http_bind,
        intercept_name: LowerName::from(Name::from_ascii(INTERCEPT_DOMAIN).expect("INTERCEPT_DOMAIN is a valid name")),
        resolver: TokioAsyncResolver::tokio(resolver_config, ResolverOpts::default()),
    };
    let mut dns_server = hickory_server::ServerFuture::new(dns_handler);

    let bind_addr = cfg.dns_addr();
    match tokio::net::UdpSocket::bind(bind_addr).await {
        Ok(socket) => {
            info!("SUCCESS: LANCache DNS listening on {}", bind_addr);
            dns_server.register_socket(socket);
            Some(dns_server)
        },
        Err(e) => {
            error!("FAILED: Could not bind to {}: {}", bind_addr, e);
            None
        }
    }
}

/// Sends a response with the given code and no records.
async fn send_empty<R: ResponseHandler>(request: &Request, mut response_handle: R, code: ResponseCode) -> ResponseInfo {
    let mut header = Header::response_from_request(request.header());
    header.set_response_code(code);
    let builder = MessageResponseBuilder::from_message_request(request);
    let response = builder.build(header, std::iter::empty(), std::iter::empty(), std::iter::empty(), std::iter::empty());
    let r_info = ResponseInfo::from(*response.header());
    response_handle.send_response(response).await.unwrap_or(r_info)
}

#[async_trait]
impl RequestHandler for LANCacheDns {
    async fn handle_request<R: ResponseHandler>(&self, request: &Request, mut response_handle: R) -> ResponseInfo {
        let query = request.request_info().query;

        if query.query_type() == RecordType::A && query.name() == &self.intercept_name {
            info!("DNS INTERCEPT: {} -> {}", query.name(), self.http_bind);
            let builder = MessageResponseBuilder::from_message_request(request);
            let header = Header::response_from_request(request.header());
            let record = Record::from_rdata(query.name().into(), 300, RData::A(A(self.http_bind)));
            let response = builder.build(header, std::iter::once(&record), std::iter::empty(), std::iter::empty(), std::iter::empty());
            let r_info = ResponseInfo::from(*response.header());
            return response_handle.send_response(response).await.unwrap_or(r_info);
        }

        // IPv4 only: never hand out IPv6 addresses. An empty NOERROR answer
        // (NODATA) tells clients the name exists but has no AAAA record, so
        // they fall back to the A record without delay.
        if query.query_type() == RecordType::AAAA {
            return send_empty(request, response_handle, ResponseCode::NoError).await;
        }

        match self.resolver.lookup(query.name(), query.query_type()).await {
            Ok(lookup) => {
                let builder = MessageResponseBuilder::from_message_request(request);
                let header = Header::response_from_request(request.header());
                let response = builder.build(header, lookup.records().iter(), std::iter::empty(), std::iter::empty(), std::iter::empty());
                let r_info = ResponseInfo::from(*response.header());
                response_handle.send_response(response).await.unwrap_or(r_info)
            }
            Err(e) => {
                // "The name exists but has no records of this type" (typically an
                // AAAA query for an IPv4-only host) must stay a NOERROR/NODATA
                // answer; reporting NXDOMAIN would tell clients the whole name
                // does not exist.
                let code = match e.kind() {
                    ResolveErrorKind::NoRecordsFound { response_code, .. } => *response_code,
                    _ => {
                        warn!("DNS lookup failed for {}: {}", query.name(), e);
                        ResponseCode::ServFail
                    }
                };
                send_empty(request, response_handle, code).await
            }
        }
    }
}
