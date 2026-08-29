// vecprobe — a CENSUS of the loops a 4-lane vectorizer could fire on.
//
// WHY IT COMES FIRST. `MEASURED` on the Graviton box: gcc -O2 emits `mla v.4s`
// on `z4_matmul_int` and is 3.3x faster than its OWN -O1 there (gcc -O1 emits
// zero SIMD), while on the `long` matmul — two lanes — gcc -O2 buys 3% and zcc's
// scalar code beats it outright at 0.904. So the prize is concentrated at four
// lanes and above, which means elements of 32 bits or narrower.
//
// zcc's MIR has NO integer vector form: `Arr` is `V2D`/`V4S` and `VAlu` carries
// an `FpOp`. A 4-lane integer vectorizer is therefore new MIR, new isel and new
// emit before any loop analysis is written — and Article A says count the sites
// first, because one benchmark is not demand. `ZCC_VECPROBE=1` reports, per
// innermost single-block counted loop:
//
//   * `elt` — the widest element any memory access in it uses, in bytes. Four
//     lanes needs 4 or less.
//   * `kind` — `map` when nothing is loop-carried but the counter, `reduce` when
//     exactly one value is, `carried` when more are (not a candidate).
//
// It is an instrument: it changes no IR and owes no square.
use super::*;

/// THEORY B — instrument half. `[candidates at <=4 bytes, reductions, maps,
/// loops examined]`, so the census can say what a 4-lane row would fire on.
pub static SEEN: [std::sync::atomic::AtomicUsize; 4] = [
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
];

pub fn wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ZCC_VECPROBE").is_some())
}

pub fn census(f: &Func, a: &mut Analyses) {
    if !wanted() {
        return;
    }
    use std::sync::atomic::Ordering::Relaxed;
    let (_c, _dt, lf) = a.all(f);
    for (li, l) in lf.loops.iter().enumerate() {
        if lf.loops.iter().any(|x| x.parent == Some(li as u32)) || l.body.len() != 1 {
            continue;
        }
        let h = l.header as usize;
        SEEN[3].fetch_add(1, Relaxed);

        // Anything opaque — a call, a volatile access — and the loop is not a
        // candidate for any vectorizer, whatever its arithmetic looks like.
        let mut elt: u32 = 0;
        let mut opaque = false;
        for inst in &f.blocks[h].insts {
            match inst {
                Inst::Load { ty, vol: false, .. } | Inst::Store { ty, vol: false, .. } => {
                    elt = elt.max(ty.bytes());
                }
                other => {
                    if !matches!(other.effect(), Effect::Pure) {
                        opaque = true;
                    }
                }
            }
        }
        if opaque || elt == 0 {
            continue;
        }

        // LOOP-CARRIED VALUES: header parameters whose back-edge argument is
        // computed inside the loop. One of them is the counter; a second is a
        // reduction; more than that is a dependence this row would not vectorize.
        let back = f.blocks[h].term.targets().iter().find(|t| t.block == l.header).cloned();
        let back = match back {
            Some(t) => t,
            None => continue,
        };
        let mut carried = 0usize;
        for (k, _p) in f.blocks[h].params.iter().enumerate() {
            if let Some(Operand::Val(v)) = back.args.get(k).copied() {
                if matches!(f.values[v as usize].def, Def::Inst(b, _) | Def::Param(b, _) if b as usize == h)
                {
                    carried += 1;
                }
            }
        }
        let kind = match carried {
            0 | 1 => "map",
            2 => "reduce",
            _ => "carried",
        };
        if kind == "carried" {
            continue;
        }
        if elt <= 4 {
            SEEN[0].fetch_add(1, Relaxed);
            if kind == "reduce" {
                SEEN[1].fetch_add(1, Relaxed);
            } else {
                SEEN[2].fetch_add(1, Relaxed);
            }
            eprintln!("[vecprobe] {} loop@b{} elt={} {}", f.name, l.header, elt, kind);
            if std::env::var_os("ZCC_VECDBG").is_some() {
                for inst in &f.blocks[h].insts {
                    eprintln!("   {:?}", inst);
                }
                eprintln!("   params={:?} term={:?}", f.blocks[h].params, f.blocks[h].term);
            }
        }
    }
}
