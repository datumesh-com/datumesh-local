//! A file on another file system than the data directory publishes (a rename across devices
//! fails with EXDEV; found by the 1.8b live rooting check). Runs where /dev/shm is a separate file
//! system, as in the test container; elsewhere it checks nothing.
#![cfg(unix)]

mod common;

use std::os::unix::fs::MetadataExt;

use common::*;

#[test]
fn a_file_on_another_file_system_publishes() {
    let up = upstream();
    let dir = TempDir::new("publish-elsewhere");
    let clock = ManualClock::new("2026-11-15T12:00:00.000000Z");
    let r = resolver_at(&dir, &up, &clock);
    let shm = std::path::Path::new("/dev/shm");
    let (Ok(a), Ok(b)) = (std::fs::metadata(shm), std::fs::metadata(dir.path())) else { return };
    if a.dev() == b.dev() {
        return;
    }
    let file = shm.join(format!("datumesh-publish-elsewhere-{}.jsonl", std::process::id()));
    std::fs::write(
        &file,
        concat!(
            r#"{"function": "local/note", "signature": {"args": {"k": {"type": "string", "valid": false}}, "order": ["k"], "returns": "string"}}"#,
            "\n",
            r#"{"cell": {"fn": "local/note", "args": {"k": {"type": "string", "value": "a"}}}, "value": {"type": "string", "value": "first"}, "tx": "2026-11-15T12:00:00.000000Z", "asof": "2026-11-15T12:00:00.000000Z", "licence": "CC0-1.0", "summary": "A note."}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = r.publish(&file);
    let _ = std::fs::remove_file(&file);
    out.unwrap();
    assert_eq!(r.get(r#"local/note(k: "a")"#).unwrap().status, 200);
}
