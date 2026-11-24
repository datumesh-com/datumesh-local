//! The localhost endpoint: the mesh's handlers (datumesh-http's router) over the local store,
//! bound to 127.0.0.1 only, with `GET /cell/...` answered by [`Resolver::get`] (so a request
//! there fetches, gates and forwards as the CLI does). Links in the mesh document and in
//! answers are `http://127.0.0.1:<port>/...`.
//!
//! The mesh document served here is the unrooted host's, on the loopback origin: its endpoint
//! URLs are `http://127.0.0.1:<port>/...`, which spec-0.5.0 allows for an unrooted host (a name
//! under .invalid) on a loopback address only (`127.0.0.1`, `[::1]`, `localhost`); rooted hosts
//! stay https-only. `/describe/{id}` is answered by [`Resolver::describe`] (16.8). The links the mesh's
//! handlers put in answers (a job's `poll`, a slice page's `next`, the `Link: rel="canonical"`
//! header) point at `https://local.invalid/...` and cannot be followed: `.invalid` never
//! resolves.
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
use datumesh_core::json::{self, Value};
use datumesh_http::Mesh;

use crate::calculus::{OverrideArg, SolveArg, WhatifArgs};
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

/// A link on the loopback origin: `url`'s scheme and host replaced by `origin` (for example
/// `https://local.invalid/jobs/abc` on `http://127.0.0.1:7733` gives `http://127.0.0.1:7733/jobs/abc`).
/// CONTRACT-1.8b L7: the `poll` and `next` links of this endpoint's answers.
pub fn loopback_link(origin: &str, url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    format!("{origin}{path}")
}

/// The endpoint's router: the mesh's handlers, `/cell` through the resolver. The `poll` and
/// `next` links of this machine's host are on the loopback origin (CONTRACT-1.8b L7); the
/// `Link: rel="canonical"` header stays as the mesh gives it.
pub fn router(resolver: Arc<Resolver>) -> Router {
    resolver
        .mesh()
        .router()
        .layer(axum::middleware::from_fn_with_state(resolver.clone(), describe))
        .layer(axum::middleware::from_fn_with_state(resolver.clone(), whatif))
        .layer(axum::middleware::from_fn_with_state(resolver.clone(), cell))
        .layer(axum::middleware::from_fn_with_state(resolver, loopback))
}

/// `POST /whatif` (SPEC 5.9's body, datumesh-local 0.5.2 item 13): the scenario of the request through the
/// resolver, as the MCP tool `whatif` gives it: `{"outcome": "scenario", "scenario": {...}}`, or the
/// refusal with its code in `error`.
async fn whatif(State(r): State<Arc<Resolver>>, req: Request, next: Next) -> Response {
    if req.method() != axum::http::Method::POST || req.uri().path() != "/whatif" {
        return next.run(req).await;
    }
    let bytes = match axum::body::to_bytes(req.into_body(), 1 << 20).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let answer = tokio::task::spawn_blocking(move || {
        let args = whatif_request(&bytes)?;
        r.whatif(&args)
    })
    .await;
    match answer {
        Ok(Ok(sc)) => {
            let body =
                Value::Object(vec![("outcome".into(), Value::String("scenario".into())), ("scenario".into(), sc)]);
            json_response(StatusCode::OK, body.to_jcs())
        }
        Ok(Err(e)) => {
            let detail = e.to_string();
            let code = crate::mcp::tool_code_of(e);
            let status = if code == "E_REQUEST" { StatusCode::BAD_REQUEST } else { StatusCode::UNPROCESSABLE_ENTITY };
            let body =
                Value::Object(vec![("error".into(), Value::String(code)), ("detail".into(), Value::String(detail))]);
            json_response(status, body.to_jcs())
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response {
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// The request of a `POST /whatif` body (SPEC 5.9): `cell` (a call), `asof`, `overrides` (each `{cell,
/// value}`, the value a typed `{"type", "value"}` or a plain literal) and `solve`.
fn whatif_request(bytes: &[u8]) -> Result<WhatifArgs, Error> {
    let bad = |m: &str| Error::Request(format!("whatif body: {m}"));
    let v = json::parse(bytes).map_err(|e| bad(&e.to_string()))?;
    let text = |x: Option<&Value>| x.and_then(Value::as_str).map(str::to_string);
    // A typed scalar's lexical form, or a plain literal as it is.
    let literal = |x: &Value| -> Option<Value> {
        match x {
            Value::Object(_) => x.get("value").and_then(Value::as_str).map(|s| Value::String(s.to_string())),
            Value::String(s) => Some(Value::String(s.clone())),
            Value::Bool(b) => Some(Value::String(b.to_string())),
            other => other.as_number_text().map(|n| Value::String(n.to_string())),
        }
    };
    let request = text(v.get("cell")).ok_or_else(|| bad("cell: a call"))?;
    let asof = match v.get("asof") {
        None => None,
        Some(Value::String(a)) => Some(a.clone()),
        Some(_) => return Err(bad("asof: an instant")),
    };
    let mut overrides = Vec::new();
    match v.get("overrides") {
        None => {}
        Some(Value::Array(items)) => {
            for o in items {
                let cell = text(o.get("cell")).ok_or_else(|| bad("overrides: each has a cell"))?;
                let value = o.get("value").and_then(literal).ok_or_else(|| bad("overrides: each has a value"))?;
                overrides.push(OverrideArg::Value { cell, value });
            }
        }
        Some(_) => return Err(bad("overrides: an array")),
    }
    let solve = match v.get("solve") {
        None => None,
        Some(sv) => {
            let need = |k: &str| sv.get(k).and_then(literal).ok_or_else(|| bad(&format!("solve.{k}")));
            Some(SolveArg {
                vary: text(sv.get("vary")).ok_or_else(|| bad("solve.vary: a call"))?,
                target: text(sv.get("target")).ok_or_else(|| bad("solve.target: a call"))?,
                equals: need("equals")?,
                lo: need("lo")?,
                hi: need("hi")?,
            })
        }
    };
    Ok(WhatifArgs { request, overrides, asof, solve })
}

/// The answer's `poll` and `next` members that name this machine's host, rewritten onto the
/// loopback origin; any other answer is as it is.
async fn loopback(State(r): State<Arc<Resolver>>, req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    let Some(origin) = r.loopback_origin() else { return res };
    let is_json = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        .is_some_and(|c| c.starts_with("application/json"));
    if !is_json {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 << 20).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let Ok(Value::Object(mut members)) = json::parse(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let mut changed = false;
    for (k, v) in members.iter_mut() {
        if let ("poll" | "next", Value::String(url)) = (k.as_str(), v) {
            let local =
                url.strip_prefix("https://").and_then(|r| r.split('/').next()) == Some(r.config().host.as_str());
            if local {
                *url = loopback_link(&origin, url);
                changed = true;
            }
        }
    }
    if !changed {
        return Response::from_parts(parts, Body::from(bytes));
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(Value::Object(members).to_jcs()))
}

/// `/cell/{rest}`: the answer through the resolver (fetch, gate, forward).
async fn cell(State(r): State<Arc<Resolver>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if req.method() != axum::http::Method::GET || !path.starts_with("/cell/") {
        return next.run(req).await;
    }
    // The request is percent-encoded as a URL path carries it: decoded once, as a request is written.
    let mut rest = percent_decode(&path["/cell/".len()..]);
    if let Some(q) = req.uri().query() {
        rest.push('?');
        rest.push_str(&percent_decode(q));
    }
    let answer = tokio::task::spawn_blocking(move || r.get(&rest)).await;
    answer_response(answer)
}

/// `/describe/{id}` (16.8): the describe answer through the resolver (local classes, else forwarded).
async fn describe(State(r): State<Arc<Resolver>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if req.method() != axum::http::Method::GET || !path.starts_with("/describe/") {
        return next.run(req).await;
    }
    let id = percent_decode(&path["/describe/".len()..]);
    let answer = tokio::task::spawn_blocking(move || r.describe(&id)).await;
    answer_response(answer)
}

/// An answer as the endpoint serves it: its status, its envelope, and its headers.
fn answer_response(answer: Result<Result<crate::Reply, Error>, tokio::task::JoinError>) -> Response {
    match answer {
        Ok(Ok(reply)) => {
            let mut resp =
                (StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), reply.body)
                    .into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(l) =
                reply.canonical.and_then(|c| HeaderValue::from_str(&format!("<{c}>; rel=\"canonical\"")).ok())
            {
                h.insert(header::LINK, l);
            }
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

/// The bytes of a percent-encoded path segment (a `%XX` is one byte; anything else is as it is).
fn percent_decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
        if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
            return Err(Error::Address(format!("{addr}: the local endpoint binds 127.0.0.1 only")));
        }
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| Error::Address(format!("{addr}: {e}")))?;
        let bound = listener.local_addr().map_err(|e| Error::Address(e.to_string()))?;
        resolver.set_base_url(Some(format!("http://{bound}")))?;
        Ok(Server { listener, resolver })
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
