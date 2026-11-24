//! datumesh-local 0.5.2, item 32 (documentation): the installed `config.json`'s comment on
//! `find_meshes` says that with `--remote` the forward mesh is asked too (ITEM32-find-sources.md:
//! `Resolver::find_in` adds `forward_mesh` to the meshes asked whenever `remote` is true).

mod common;

use common::*;
use datumesh_local::Config;

#[test]
fn the_find_meshes_comment_says_the_forward_mesh_is_asked_with_remote() {
    let dir = TempDir::new("l052-find-doc");
    Config::in_dir(dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| l.trim_start().starts_with("\"find_meshes\"")).expect("a find_meshes member");
    // The comment block right above the member.
    let comment: Vec<&str> =
        lines[..at].iter().rev().take_while(|l| l.trim_start().starts_with("//")).copied().collect();
    let comment = comment.into_iter().rev().collect::<Vec<_>>().join(" ").to_lowercase();
    assert!(comment.contains("--remote"), "{comment}");
    assert!(
        comment.contains("forward_mesh") || comment.contains("forward mesh"),
        "says the forward mesh is asked too with --remote: {comment}"
    );
}
