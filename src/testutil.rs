// Shared test scaffolding: run the surviving frontend on a source string so a
// battery can state its case as C, not as hand-built IR. The C standard is the
// oracle; zcc is never asked what it currently does.
use crate::ast::Ast;
use crate::{parser, preprocess};

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn frontend(src: &str) -> Ast {
    let dir = std::env::temp_dir().join("zcc-battery");
    std::fs::create_dir_all(&dir).unwrap();
    // name the file after the source so concurrent test threads cannot collide
    let mut h: u64 = 1469598103934665603;
    for b in src.as_bytes() {
        h = (h ^ *b as u64).wrapping_mul(1099511628211);
    }
    let path = dir.join(format!("t{:016x}.c", h));
    // Two batteries may quote the SAME program, so the hash names ONE file that
    // several test threads write at once — and `fs::write` truncates before it
    // writes, so a concurrent reader can preprocess half a file ("no f" in a
    // battery that had nothing to do with the writer). Write a private file and
    // RENAME it into place: rename is atomic, and since the name is the content
    // hash, whichever writer wins leaves the same bytes.
    let tmp = dir.join(format!(
        "t{:016x}.{}.{}.tmp",
        h,
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, src).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    let p = path.to_str().unwrap().to_string();
    let incs = vec!["\u{1}nostdinc".to_string()];
    let (toks, locs, files) = preprocess::preprocess(&p, &[], &[], &incs).expect("preprocess");
    parser::parse(&toks, &locs, &files).expect("parse")
}
