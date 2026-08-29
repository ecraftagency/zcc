// copyprobe — a CENSUS, not a transform (Article A: demand is DETECTED).
//
// `perf` on the 96-program suite says zcc executes 1.3250x the instructions of
// `gcc -O2` while emitting 1.0045x the static ones, and the largest single
// outlier is `g1_memcpy_loop` at 30.2x — an element-at-a-time copy loop that gcc
// turns into `bl memcpy`. Before any of that is built, Article A asks how many
// times the shape actually occurs in real code: "a shape in the suite is not
// evidence the shape occurs".
//
// WHAT IT COUNTS. A loop whose body is exactly one load and one store, the store
// writing exactly the value the load produced, at the same width, with nothing
// else that reads or writes memory and no call. That is the copy signature; the
// address form is reported separately rather than required, because a census that
// demands the final transform's full side conditions cannot tell "the shape is
// absent" from "my recognizer is narrow".
//
// `ZCC_COPYPROBE=1` prints one line per candidate and a total. It is an
// instrument: it changes no IR and is never on in a shipped compile.
use super::*;

/// THEORY B — instrument half. Not a value the compiler computes with: it is what
/// lets the census ask how many copy loops a translation unit holds, which is the
/// question Article A puts before the transform is written.
pub static FOUND: [std::sync::atomic::AtomicUsize; 3] = [
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
];

pub fn wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ZCC_COPYPROBE").is_some())
}

/// The census itself. NOT `run`: this is not a pass and owes no commuting
/// square, because it rewrites nothing — `tests/provenance.sh` asks a square of
/// every `pub fn run` in `hir/pass/`, and it is right to.
pub fn census(f: &Func, a: &mut Analyses) {
    if !wanted() {
        return;
    }
    use std::sync::atomic::Ordering::Relaxed;
    let (_c, _dt, lf) = a.all(f);
    for (li, l) in lf.loops.iter().enumerate() {
        // Innermost only: an outer loop containing a copy loop is not itself one.
        if lf.loops.iter().any(|x| x.parent == Some(li as u32)) {
            continue;
        }
        FOUND[2].fetch_add(1, Relaxed);
        let mut loads: Vec<(Ty, Operand, ValueId)> = Vec::new();
        let mut stores: Vec<(Ty, Operand, Operand)> = Vec::new();
        let mut opaque = false;
        for &b in &l.body {
            for inst in &f.blocks[b as usize].insts {
                match inst {
                    Inst::Load { dst, ty, addr, vol: false, .. } => {
                        loads.push((*ty, *addr, *dst))
                    }
                    Inst::Store { ty, addr, val, vol: false, .. } => {
                        stores.push((*ty, *addr, *val))
                    }
                    other => {
                        if !matches!(other.effect(), Effect::Pure) {
                            opaque = true;
                        }
                    }
                }
            }
        }
        if std::env::var_os("ZCC_COPYDBG").is_some() {
            eprintln!(
                "[copydbg] {} loop@b{} loads={} stores={} opaque={}",
                f.name, l.header, loads.len(), stores.len(), opaque
            );
            for &b in &l.body {
                for inst in &f.blocks[b as usize].insts {
                    eprintln!("   b{} {:?}", b, inst);
                }
            }
        }
        if opaque || loads.len() != 1 || stores.len() != 1 {
            continue;
        }
        let (lty, laddr, ldst) = loads[0];
        let (sty, saddr, sval) = stores[0];
        // The store writes exactly what the load read, at the same width: that is
        // the copy, and nothing weaker is one.
        if lty != sty || sval != Operand::Val(ldst) || laddr == saddr {
            continue;
        }
        FOUND[0].fetch_add(1, Relaxed);
        if lty.bytes() == 1 {
            FOUND[1].fetch_add(1, Relaxed);
        }
        eprintln!(
            "[copyprobe] {} loop@b{} width={} blocks={}",
            f.name,
            l.header,
            lty.bytes(),
            l.body.len()
        );
    }
}
