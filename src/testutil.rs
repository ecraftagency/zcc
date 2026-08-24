// Shared test scaffolding: run the surviving frontend on a source string so a
// battery can state its case as C, not as hand-built IR. The C standard is the
// oracle; zcc is never asked what it currently does.
use crate::ast::Ast;
use crate::{parser, preprocess};

pub fn frontend(src: &str) -> Ast {
    let dir = std::env::temp_dir().join("zcc-battery");
    std::fs::create_dir_all(&dir).unwrap();
    // name the file after the source so concurrent test threads cannot collide
    let mut h: u64 = 1469598103934665603;
    for b in src.as_bytes() {
        h = (h ^ *b as u64).wrapping_mul(1099511628211);
    }
    let path = dir.join(format!("t{:016x}.c", h));
    std::fs::write(&path, src).unwrap();
    let p = path.to_str().unwrap().to_string();
    let incs = vec!["\u{1}nostdinc".to_string()];
    let (toks, locs, files) = preprocess::preprocess(&p, &[], &[], &incs).expect("preprocess");
    parser::parse(&toks, &locs, &files).expect("parse")
}
