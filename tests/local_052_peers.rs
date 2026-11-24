//! datumesh-local 0.5.2, item 31: a `peers` origin must start with `http://` or `https://`. Anything
//! else is refused when the configuration loads, naming the entry, as the mesh does.

mod common;

use common::*;
use datumesh_local::Config;

fn home_with_peer(host: &str, origin: &str) -> TempDir {
    let dir = TempDir::new("l052-peers");
    Config::in_dir(dir.path()).unwrap();
    let p = dir.path().join("config.json");
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("\"peers\": {}"));
    std::fs::write(&p, text.replace("\"peers\": {}", &format!("\"peers\": {{\"{host}\": \"{origin}\"}}"))).unwrap();
    dir
}

#[test]
fn an_origin_without_http_or_https_is_refused_at_load_naming_the_entry() {
    for origin in ["ftp://x", "127.0.0.1:8797", "file:///", "localhost:8797", "", "//publisher.example"] {
        let dir = home_with_peer("publisher.example", origin);
        let e = Config::in_dir(dir.path()).err().unwrap_or_else(|| panic!("{origin:?} is refused at load"));
        let text = e.to_string();
        assert!(text.contains("peers") && text.contains("publisher.example"), "{origin:?}: names the entry: {text}");
        if !origin.is_empty() {
            assert!(text.contains(origin), "{origin:?}: names the origin: {text}");
        }
    }
}

#[test]
fn http_and_https_origins_load_as_before() {
    for origin in ["http://127.0.0.1:8797", "https://mesh.publisher.example"] {
        let dir = home_with_peer("publisher.example", origin);
        let c = Config::in_dir(dir.path()).unwrap_or_else(|e| panic!("{origin}: {e}"));
        assert_eq!(c.peers.get("publisher.example").map(String::as_str), Some(origin));
    }
}
