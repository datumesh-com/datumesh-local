//! datumesh-local 0.5.4, CONTRACT-patch-0.6.1 item 4c: the local client follows `poll`.
//! Written first; FAIL until then.
//!
//! When a mesh answers 202 `pending` with a `poll` URL, datumesh-local polls that URL, waiting the answer's
//! estimate (at least 1 s, at most 10 s between polls); stops at its own deadline with the mesh's answer; never
//! re-sends the original `/cell` request while the job is known; on `E_JOB_UNKNOWN` (a restart) sends the
//! original request once more.
//!
//! Fixture: a stub mesh in front of the test mesh (`common::Upstream`, reached as `mesh-test.datumesh.com`
//! through `peers`). For the `/cell` requests of one cell (a marker in the URL: the stored foreign cell `USD`, or
//! the forwarded computation `NOK_FULL`) it answers by a script: `pending` (a valid 5.5 envelope echoing the
//! request: the real answer's cell id, the request's `asof` and pin) or the real answer; `/jobs/<JOB>` answers
//! `pending`, `E_JOB_UNKNOWN` or the real answer. It counts and times the hits of each. Every other path is the
//! test mesh's.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use datumesh_core::json::{self, Value};
use datumesh_core::{Pending, Receipt, Request};

const JOB: &str = "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";
const NOK_FULL: &str = r#"mesh-test.datumesh.com/ecb/fx_cross(base: "USD", quote: "NOK", date: 2026-10-30)"#;

/// What the stub answers.
#[derive(Clone, Copy)]
struct Plan {
    /// The first this many `/cell` requests for the cell are answered `pending`; later ones with the real answer.
    cell_pending: u32,
    /// The first this many polls are answered `pending`; later ones with the real answer.
    jobs_pending: u32,
    /// Every poll is answered 400 `E_JOB_UNKNOWN`.
    unknown: bool,
    /// The `estimate_seconds` of each `pending`.
    estimate: u64,
}

#[derive(Default)]
struct Hits {
    cell: Vec<Instant>,
    jobs: Vec<Instant>,
}

struct Stub {
    origin: String,
    hits: Arc<Mutex<Hits>>,
    _rt: tokio::runtime::Runtime,
}

impl Stub {
    fn cell_hits(&self) -> usize {
        self.hits.lock().unwrap().cell.len()
    }
    fn job_hits(&self) -> usize {
        self.hits.lock().unwrap().jobs.len()
    }
    /// The gaps between consecutive hits (`/cell` and `/jobs` together, in time order).
    fn gaps(&self) -> Vec<Duration> {
        let h = self.hits.lock().unwrap();
        let mut all: Vec<Instant> = h.cell.iter().chain(h.jobs.iter()).copied().collect();
        all.sort();
        all.windows(2).map(|w| w[1] - w[0]).collect()
    }
}

fn query_param(query: &str, k: &str) -> Option<String> {
    query.split('&').find_map(|p| p.strip_prefix(&format!("{k}="))).map(|v| v.replace("%3A", ":").replace("%3a", ":"))
}

fn respond(status: u16, body: Vec<u8>) -> axum::response::Response {
    axum::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// The real answer and the `pending` envelope of the cell.
type Answers = (Vec<u8>, Vec<u8>);

/// The stub in front of `up` for `/cell` requests whose path and query contain every one of `marker`.
fn stub(up: &Upstream, marker: &'static [&'static str], plan: Plan) -> Stub {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(Hits::default()));
    // The real answer and the pending envelope, from the first /cell request.
    let state: Arc<Mutex<Option<Answers>>> = Arc::new(Mutex::new(None));
    let (h, st) = (hits.clone(), state.clone());
    let layer = axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
        let (h, st) = (h.clone(), st.clone());
        async move {
            let path = req.uri().path().to_string();
            let query = req.uri().query().unwrap_or("").to_string();
            let whole = format!("{path}?{query}");
            if path == format!("/jobs/{JOB}") {
                let n = {
                    let mut h = h.lock().unwrap();
                    h.jobs.push(Instant::now());
                    h.jobs.len() as u32
                };
                if plan.unknown {
                    return respond(400, br#"{"error":"E_JOB_UNKNOWN","outcome":"bad_request"}"#.to_vec());
                }
                let (real, pending) = st.lock().unwrap().clone().expect("a poll before the /cell request");
                return if n <= plan.jobs_pending { respond(202, pending) } else { respond(200, real) };
            }
            if !(path.starts_with("/cell/") && marker.iter().all(|m| whole.contains(m))) {
                return next.run(req).await;
            }
            let n = {
                let mut h = h.lock().unwrap();
                h.cell.push(Instant::now());
                h.cell.len() as u32
            };
            let res = next.run(req).await;
            let status = res.status().as_u16();
            let real = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap().to_vec();
            assert_eq!(status, 200, "the test mesh's answer: {}", String::from_utf8_lossy(&real));
            let env = json::parse(&real).unwrap();
            let receipt = Receipt::from_json(&env.get("receipt").unwrap().to_jcs()).unwrap();
            let mut request = Request::new(&receipt.cell().id(), query_param(&query, "asof").as_deref()).unwrap();
            if let Some(pin) = query_param(&query, "fn_hash") {
                request = request.with_pin(&pin).unwrap();
            }
            let pending =
                Pending::new(JOB, plan.estimate, &format!("https://{HOST}/jobs/{JOB}"), request).unwrap().to_json();
            *st.lock().unwrap() = Some((real.clone(), pending.clone()));
            if n <= plan.cell_pending {
                respond(202, pending)
            } else {
                respond(200, real)
            }
        }
    });
    let router = up.mesh.router().layer(layer);
    rt.spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Stub { origin: format!("http://{addr}"), hits, _rt: rt }
}

/// A resolver reaching the test mesh through `stub`, with `timeout` as its own deadline.
fn through(dir: &TempDir, up: &Upstream, s: &Stub, timeout: Duration) -> datumesh_local::Resolver {
    let mut c = config(dir, up);
    c.peers.insert(HOST.to_string(), s.origin.clone());
    c.timeout = timeout;
    datumesh_local::Resolver::open(c).unwrap()
}

const USD_MARK: &[&str] = &["fx_rate", "USD"];
const NOK_MARK: &[&str] = &["fx_cross", "NOK"];

fn verified_ok(up: &Upstream, status: u16, body: &[u8]) {
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(body));
    let env = envelope(body);
    assert_eq!(env.get("outcome").and_then(Value::as_str), Some("ok"), "{}", String::from_utf8_lossy(body));
    let rc = receipt(body);
    let v = datumesh_core::verify(&rc.to_jcs(), &upstream_keys(up), None, None);
    assert!(v.ok, "{:?}: {}", v.code, v.detail);
}

/// Item 4c: a 202 with `poll` is followed: the poll URL is polled until the job is done, and the answer is the
/// job's (verified, held). The original `/cell` request is sent once: never again while the job is known.
/// Today: the 202 `pending` is handed back as it came.
#[test]
fn l054_4c_a_pending_answer_is_followed_to_the_jobs_answer() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: 1, jobs_pending: 2, unknown: false, estimate: 1 });
    let dir = TempDir::new("l054-follow");
    let r = through(&dir, &up, &s, Duration::from_secs(20));
    let a = r.get(USD).unwrap();
    verified_ok(&up, a.status, &a.body);
    assert_eq!(s.cell_hits(), 1, "the original request is not re-sent while the job is known");
    assert_eq!(s.job_hits(), 3, "polled until done");
    // Held: the second answer makes no request.
    let b = r.get(USD).unwrap();
    assert_eq!(b.body, a.body);
    assert_eq!((s.cell_hits(), s.job_hits()), (1, 3));
}

/// Item 4c for a forwarded computation (the licence gate's forward to the forward mesh): the same rule.
/// Today: the 202 `pending` is handed back as it came.
#[test]
fn l054_4c_a_forwarded_computation_follows_poll_too() {
    let up = upstream();
    let s = stub(&up, NOK_MARK, Plan { cell_pending: 1, jobs_pending: 1, unknown: false, estimate: 1 });
    let dir = TempDir::new("l054-forward");
    let r = through(&dir, &up, &s, Duration::from_secs(20));
    let a = r.get(NOK_FULL).unwrap();
    verified_ok(&up, a.status, &a.body);
    assert_eq!((s.cell_hits(), s.job_hits()), (1, 2));
}

/// Item 4c, the wait: the answer's estimate, at least 1 s: an estimate of 0 still waits a second before each poll.
/// Today: no poll at all.
#[test]
fn l054_4c_the_wait_is_at_least_one_second() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: 1, jobs_pending: 2, unknown: false, estimate: 0 });
    let dir = TempDir::new("l054-least");
    let r = through(&dir, &up, &s, Duration::from_secs(20));
    let a = r.get(USD).unwrap();
    verified_ok(&up, a.status, &a.body);
    assert_eq!((s.cell_hits(), s.job_hits()), (1, 3));
    for g in s.gaps() {
        assert!(g >= Duration::from_millis(950), "a poll {g:?} after the last answer");
    }
}

/// Item 4c, the wait: the answer's estimate, at most 10 s: an estimate of 30 waits 10 s (with a deadline of 30 s).
/// Today: no poll at all.
#[test]
fn l054_4c_the_wait_is_at_most_ten_seconds() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: 1, jobs_pending: 0, unknown: false, estimate: 30 });
    let dir = TempDir::new("l054-most");
    let r = through(&dir, &up, &s, Duration::from_secs(30));
    let a = r.get(USD).unwrap();
    verified_ok(&up, a.status, &a.body);
    assert_eq!((s.cell_hits(), s.job_hits()), (1, 1));
    let g = s.gaps();
    assert_eq!(g.len(), 1);
    assert!(g[0] >= Duration::from_millis(9500) && g[0] <= Duration::from_millis(11500), "waited {:?}", g[0]);
}

/// Item 4c, a restart: `E_JOB_UNKNOWN` from the poll sends the original request once more, and its answer is used.
/// Today: no poll at all.
#[test]
fn l054_4c_job_unknown_sends_the_original_request_once_more() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: 1, jobs_pending: 0, unknown: true, estimate: 1 });
    let dir = TempDir::new("l054-unknown");
    let r = through(&dir, &up, &s, Duration::from_secs(20));
    let a = r.get(USD).unwrap();
    verified_ok(&up, a.status, &a.body);
    assert_eq!((s.cell_hits(), s.job_hits()), (2, 1));
}

/// Item 4c, a restart, only once: when the request sent again is pending again and its job is unknown again, the
/// client stops with `E_JOB_UNKNOWN` (the mesh's code): two `/cell` requests in all, never a third.
/// Today: no poll at all (the first `pending` is handed back).
#[test]
fn l054_4c_job_unknown_resends_only_once() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: u32::MAX, jobs_pending: 0, unknown: true, estimate: 1 });
    let dir = TempDir::new("l054-unknown-twice");
    let r = through(&dir, &up, &s, Duration::from_secs(20));
    let code = match r.get(USD) {
        Ok(a) => {
            assert_ne!(a.status, 200, "{}", String::from_utf8_lossy(&a.body));
            envelope(&a.body).get("error").and_then(Value::as_str).map(str::to_string)
        }
        Err(datumesh_local::Error::Client { code, .. }) => Some(code),
        Err(e) => panic!("{e}"),
    };
    assert_eq!(code.as_deref(), Some("E_JOB_UNKNOWN"));
    assert_eq!((s.cell_hits(), s.job_hits()), (2, 2));
}

/// Item 4c, the deadline: a job that never completes is polled until the client's own deadline (its `timeout`,
/// 3 s here, counted from the first request), then the client stops with the mesh's answer: the `pending` envelope,
/// its job and `poll` as the mesh gave them. The `/cell` request is sent once.
/// Today: the first `pending` is handed back at once, with no poll.
#[test]
fn l054_4c_stops_at_its_own_deadline_with_the_meshs_answer() {
    let up = upstream();
    let s = stub(&up, USD_MARK, Plan { cell_pending: u32::MAX, jobs_pending: u32::MAX, unknown: false, estimate: 1 });
    let dir = TempDir::new("l054-deadline");
    let r = through(&dir, &up, &s, Duration::from_secs(3));
    let t0 = Instant::now();
    let a = r.get(USD).unwrap();
    let took = t0.elapsed();
    assert!(s.job_hits() >= 1, "polled before the deadline");
    assert_eq!(s.cell_hits(), 1);
    assert!(took >= Duration::from_millis(2500) && took < Duration::from_secs(6), "stopped after {took:?}");
    assert_eq!(a.status, 202, "{}", String::from_utf8_lossy(&a.body));
    let env = envelope(&a.body);
    assert_eq!(env.get("outcome").and_then(Value::as_str), Some("pending"));
    assert_eq!(env.get("job").and_then(Value::as_str), Some(JOB));
}
