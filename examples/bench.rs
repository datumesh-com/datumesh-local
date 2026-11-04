//! Brief test 7, the memory hierarchy benchmark: 500 cell reads typical of an agent task (FX
//! rates, TARGET holidays and Quran verses of the live test mesh, a working set read again
//! and again), run three ways:
//!
//! - **cold**: a resolver with an empty store (each cell fetched once, then local);
//! - **warm**: a resolver after `prefetch` of the three slices (the prefetch is reported
//!   apart), then the reads offline;
//! - **mesh**: every read a `GET /cell/...` to the mesh.
//!
//! Every request to the mesh goes through a counting proxy on 127.0.0.1 (the resolver reaches
//! the mesh through `peers`), which also keeps the requests under the mesh's rate limit (60 a
//! minute per IP, burst 120) by waiting before forwarding when needed; that waiting is
//! reported, and taken out of the wall times given as "net". Read-only.
//!
//!     cargo run --release --example bench [-- <out.json>]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use datumesh_http::client::{Client, ClientConfig};
use datumesh_local::{Config, Resolver, Source};

const MESH: &str = "mesh-test.datumesh.com";

#[derive(Default)]
struct Counter {
    requests: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    waited_us: AtomicU64,
    times: Mutex<Vec<Instant>>,
}

impl Counter {
    fn snapshot(&self) -> (u64, u64, u64, Duration) {
        (
            self.requests.load(Ordering::SeqCst),
            self.bytes_in.load(Ordering::SeqCst),
            self.bytes_out.load(Ordering::SeqCst),
            Duration::from_micros(self.waited_us.load(Ordering::SeqCst)),
        )
    }

    /// Waits until a request keeps the last minute under 55 and the last 2 s under 4.
    fn pace(&self) {
        let start = Instant::now();
        loop {
            {
                let mut t = self.times.lock().unwrap();
                let now = Instant::now();
                t.retain(|x| now.duration_since(*x) < Duration::from_secs(61));
                let recent = t.iter().filter(|x| now.duration_since(**x) < Duration::from_secs(2)).count();
                if t.len() < 55 && recent < 4 {
                    t.push(now);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.waited_us.fetch_add(start.elapsed().as_micros() as u64, Ordering::SeqCst);
    }
}

async fn proxy(State((c, client)): State<(Arc<Counter>, Client)>, req: Request) -> Response {
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
    let post = req.method() == axum::http::Method::POST;
    let body = axum::body::to_bytes(req.into_body(), 64 << 20).await.unwrap_or_default().to_vec();
    let c2 = c.clone();
    let out = tokio::task::spawn_blocking(move || {
        c2.pace();
        c2.requests.fetch_add(1, Ordering::SeqCst);
        c2.bytes_out.fetch_add((path.len() + body.len()) as u64, Ordering::SeqCst);
        let url = format!("https://{MESH}{path}");
        if post {
            panic!("the benchmark sends no POST");
        }
        client.get(&url, 64 << 20)
    })
    .await
    .unwrap();
    match out {
        Ok((status, body)) => {
            c.bytes_in.fetch_add(body.len() as u64, Ordering::SeqCst);
            Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        }
        Err(e) => Response::builder().status(502).body(Body::from(e)).unwrap(),
    }
}

/// The 500 reads: a deterministic agent-like mix over a working set, with repeats.
fn reads() -> Vec<String> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let quotes = ["USD", "GBP", "JPY", "CHF"];
    let days: Vec<String> = {
        let mut d = Vec::new();
        for (m, lo, hi) in [(9, 21, 30), (10, 1, 30)] {
            for day in lo..=hi {
                let s = format!("2026-{m:02}-{day:02}");
                // Weekdays of the seed: Sep 21 2026 is a Monday.
                let n = if m == 9 { day - 21 } else { 10 + day - 1 };
                if n % 7 < 5 {
                    d.push(s);
                }
            }
        }
        d
    };
    // The working set: 40 FX cells, 30 holidays, 30 verses.
    let mut fx = Vec::new();
    for i in 0..40 {
        fx.push(format!(
            r#"ecb/fx_rate(base: "EUR", quote: "{}", date: {})"#,
            quotes[i % 4],
            days[(i * 7 + i / 4) % days.len()]
        ));
    }
    let mut hol = Vec::new();
    for i in 0..30u32 {
        let doy = 1 + (i * 12) % 365;
        let d = chrono_like(doy);
        hol.push(format!(r#"ecb/holiday(calendar: "TARGET", date: {d})"#));
    }
    let mut verses = Vec::new();
    for (s, v) in [
        (1, 1),
        (1, 2),
        (1, 5),
        (1, 7),
        (2, 255),
        (2, 286),
        (3, 18),
        (4, 1),
        (5, 3),
        (6, 1),
        (7, 54),
        (9, 129),
        (12, 4),
        (13, 28),
        (17, 1),
        (18, 10),
        (19, 1),
        (20, 14),
        (24, 35),
        (36, 1),
        (36, 83),
        (55, 13),
        (67, 1),
        (93, 5),
        (94, 6),
        (103, 1),
        (112, 1),
        (113, 1),
        (114, 1),
        (114, 6),
    ] {
        verses.push(format!(
            r#"scripture/verse_text(corpus: "quran", book: "{s}", chapter: 1, verse: {v}, edition: "uthmani")"#
        ));
    }
    // 40% FX, 30% holidays, 30% verses; within each set, skewed to its first items
    // (index = len * u^2, u uniform): some cells read many times, some once.
    let mut out = Vec::new();
    for _ in 0..500 {
        let k = next(10);
        let u = next(1_000_000) as f64 / 1_000_000.0;
        let pick = |set: &Vec<String>| set[((set.len() as f64) * u * u) as usize].clone();
        out.push(match k {
            0..=3 => pick(&fx),
            4..=6 => pick(&hol),
            _ => pick(&verses),
        });
    }
    out
}

/// The date of day-of-year `doy` in 2026.
fn chrono_like(doy: u32) -> String {
    let months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut d = doy;
    for (i, m) in months.iter().enumerate() {
        if d <= *m {
            return format!("2026-{:02}-{:02}", i + 1, d);
        }
        d -= m;
    }
    "2026-12-31".into()
}

struct Run {
    name: &'static str,
    requests: u64,
    bytes_in: u64,
    bytes_out: u64,
    wall: Duration,
    waited: Duration,
    ok: usize,
}

fn measure(name: &'static str, c: &Counter, f: impl FnOnce() -> usize) -> Run {
    let (r0, i0, o0, w0) = c.snapshot();
    let t = Instant::now();
    let ok = f();
    let wall = t.elapsed();
    let (r1, i1, o1, w1) = c.snapshot();
    Run { name, requests: r1 - r0, bytes_in: i1 - i0, bytes_out: o1 - o0, wall, waited: w1 - w0, ok }
}

fn resolver(dir: &std::path::Path, origin: &str) -> Resolver {
    let _ = std::fs::remove_dir_all(dir);
    let mut c = Config::in_dir(dir).unwrap();
    c.peers.insert(MESH.into(), origin.into());
    Resolver::open(c).unwrap()
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let counter = Arc::new(Counter::default());
    let mut cc = ClientConfig::default();
    cc.allowed = Some(vec![MESH.into()]);
    cc.timeout = Duration::from_secs(60);
    let upstream = Client::new(cc);
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(proxy).with_state((counter.clone(), upstream));
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });

    let reads = reads();
    let distinct: std::collections::BTreeSet<&String> = reads.iter().collect();
    let kinds = |p: &str| reads.iter().filter(|r| r.starts_with(p)).count();
    eprintln!(
        "{} reads ({} FX, {} holidays, {} verses), {} distinct cells",
        reads.len(),
        kinds("ecb/fx"),
        kinds("ecb/hol"),
        kinds("scripture/"),
        distinct.len()
    );
    let base = std::env::temp_dir().join(format!("datumesh-bench-{}", std::process::id()));

    // Cold.
    let cold_r = resolver(&base.join("cold"), &origin);
    let cold = measure("cold", &counter, || {
        reads.iter().filter(|q| cold_r.get(q).map(|r| r.status == 200).unwrap_or(false)).count()
    });

    // Warm: prefetch (reported apart), then offline.
    let warm_r = resolver(&base.join("warm"), &origin);
    let prefetch = measure("prefetch", &counter, || {
        let mut n = 0;
        for s in [
            "ecb/fx_rate(base: *, quote: *, date: *)",
            "ecb/holiday(calendar: *, date: *)",
            r#"scripture/verse_text(corpus: "quran", book: *, chapter: *, verse: *, edition: "uthmani")"#,
        ] {
            n += warm_r.prefetch(s, Some(10_000), false).unwrap().members;
        }
        n
    });
    warm_r.set_offline(true);
    let warm = measure("warm", &counter, || {
        reads
            .iter()
            .filter(|q| warm_r.get(q).map(|r| r.status == 200 && r.source == Source::Local).unwrap_or(false))
            .count()
    });

    // Direct to the mesh: GET /cell/<call>, through the proxy.
    let mut cc = ClientConfig::default();
    cc.peers.insert(MESH.into(), origin.clone());
    cc.allowed = Some(vec![MESH.into()]);
    cc.timeout = Duration::from_secs(60);
    let direct = Client::new(cc);
    let mesh = measure("mesh", &counter, || {
        reads
            .iter()
            .filter(|q| {
                let full = q.replacen("ecb/", "mesh-test.datumesh.com/ecb/", 1).replacen(
                    "scripture/",
                    "mesh-test.datumesh.com/scripture/",
                    1,
                );
                let url = format!("https://{MESH}/cell/{}", datumesh_local::serve::encode_path(&full));
                matches!(direct.get(&url, 64 << 20), Ok((200, _)))
            })
            .count()
    });
    let _ = std::fs::remove_dir_all(&base);

    let row = |r: &Run| {
        let net = r.wall.saturating_sub(r.waited);
        format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.2} | {}/{} |",
            r.name,
            r.requests,
            r.bytes_in + r.bytes_out,
            (r.bytes_in + r.bytes_out) / 4,
            r.bytes_in,
            r.wall.as_secs_f64(),
            r.waited.as_secs_f64(),
            net.as_secs_f64(),
            r.ok,
            if r.name == "prefetch" { r.ok } else { reads.len() },
        )
    };
    let mut table = String::new();
    table.push_str("| run | requests | bytes (in+out) | tokens-equivalent (bytes/4) | bytes in | wall s | rate-limit wait s | net wall s | answers ok |\n");
    table.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for r in [&cold, &prefetch, &warm, &mesh] {
        table.push_str(&row(r));
        table.push('\n');
    }
    println!("{table}");
    if let Some(out) = std::env::args().nth(1).or_else(|| std::env::var("DATUMESH_BENCH_OUT").ok()) {
        std::fs::write(out, &table).unwrap();
    }
}
