//! The localhost endpoint: the mesh's handlers (datumesh-http's router) over the local store,
//! bound to 127.0.0.1 only, with `GET /cell/...` answered by [`Resolver::get`] (so a request
//! there fetches, gates and forwards as the CLI does). Links in the mesh document and in
//! answers are `http://127.0.0.1:<port>/...`.
//!
//! A `/cell` answer whose receipt this machine signed carries the header
//! `Datumesh-Signer: local, verifies only on this machine`; an answer that is not the cell's
//! (`E_FOREIGN_UNAVAILABLE`, a forwarded answer) says why in `Datumesh-Detail`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use datumesh_http::Mesh;

use crate::{Error, Resolver};

/// Percent-encodes what a URI path cannot carry as itself (spaces, quotes, non-ASCII).
pub fn encode_path(rest: &str) -> String {
    let mut out = String::with_capacity(rest.len());
    for b in rest.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/()*,:;=@?&%+$!'".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Runs one `GET path` through the mesh's router, in process.
pub(crate) fn oneshot(mesh: &Mesh, path: &str) -> Result<(u16, Vec<u8>), Error> {
    use tower::ServiceExt;
    let router = mesh.router();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Request(format!("runtime: {e}")))?;
    rt.block_on(async move {
        let mut req =
            Request::builder().uri(path).body(Body::empty()).map_err(|e| Error::Request(format!("{path}: {e}")))?;
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let resp = router.oneshot(req).await.map_err(|e| Error::Request(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = axum::body::to_bytes(resp.into_body(), 64 << 20).await.map_err(|e| Error::Request(e.to_string()))?;
        Ok((status, body.to_vec()))
    })
}

/// The endpoint's router: the mesh's handlers, `/cell` through the resolver.
pub fn router(resolver: Arc<Resolver>) -> Router {
    resolver.mesh().router().layer(axum::middleware::from_fn_with_state(resolver, cell))
}

async fn cell(State(r): State<Arc<Resolver>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if req.method() != axum::http::Method::GET || !path.starts_with("/cell/") {
        return next.run(req).await;
    }
    let mut rest = path["/cell/".len()..].to_string();
    if let Some(q) = req.uri().query() {
        rest.push('?');
        rest.push_str(q);
    }
    let answer = tokio::task::spawn_blocking(move || r.get(&rest)).await;
    match answer {
        Ok(Ok(reply)) => {
            let mut resp =
                (StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), reply.body)
                    .into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(l) = reply.label.and_then(|l| HeaderValue::from_str(&l).ok()) {
                h.insert("datumesh-signer", l);
            }
            if let Some(d) = reply.detail.and_then(|d| HeaderValue::from_str(&d.replace(['\r', '\n'], " ")).ok()) {
                h.insert("datumesh-detail", d);
            }
            resp
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// The endpoint, bound.
pub struct Server {
    listener: tokio::net::TcpListener,
    resolver: Arc<Resolver>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("addr", &self.listener.local_addr().ok()).finish()
    }
}

impl Server {
    /// Binds `addr`, which must be 127.0.0.1 (any other address is refused with
    /// [`Error::Address`]); port 0 picks one. The resolver's links become
    /// `http://127.0.0.1:<port>`.
    pub async fn bind(resolver: Arc<Resolver>, addr: SocketAddr) -> Result<Server, Error> {
        Err(Error::Request("not implemented yet".into()))
    }

    /// The address bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener")
    }

    /// Serves until `shutdown` completes.
    pub async fn run(self, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
        let app = router(self.resolver);
        axum::serve(self.listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(shutdown)
            .await
    }
}
