// AAPCS64 parameter passing (IHI 0055 §6.4–6.8, the C.1–C.15 automaton).
// THEORY II-3 is the transcribed table; this file is the algorithm over it.
//
// R0 realizes the scalar subset of the automaton — stages A/B trivial, C.1
// (HFA/HVA), C.10 (composites) and the variadic rules land in R1.2. The state
// (NGRN, NSRN, NSAA) is the spec's, not an approximation of it, so widening the
// rule set never rewrites the caller.
use crate::hir::{PTy, Sig};
use crate::mir::{PReg, Width};

/// Where one argument (or the return value) is passed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Loc {
    Reg(PReg, Width),
    /// byte offset into the outgoing-argument area (NSAA-relative)
    Stack(u32, Width),
}

pub struct Assign {
    pub args: Vec<Loc>,
    pub ret: Option<Loc>,
    /// NSAA at the end: the size of the outgoing stack-argument area
    pub stack_bytes: u32,
}

fn width(p: &PTy) -> Width {
    match p {
        PTy::S(t) => match t {
            crate::hir::Ty::F32 => Width::S,
            crate::hir::Ty::F64 => Width::D,
            crate::hir::Ty::I64 => Width::W64,
            _ => Width::W32,
        },
        // R1.2: a 16-byte binary128 or a composite
        PTy::LDouble | PTy::Agg { .. } => Width::W64,
    }
}

/// The C.1–C.15 walk. `ngrn`/`nsrn` are the next general/SIMD register numbers,
/// `nsaa` the next stack-argument address — exactly the spec's three counters.
pub fn classify(sig: &Sig) -> Assign {
    let (mut ngrn, mut nsrn, mut nsaa) = (0u8, 0u8, 0u32);
    let mut args = Vec::with_capacity(sig.params.len());
    for p in &sig.params {
        let w = width(p);
        match p {
            PTy::S(t) if t.is_float() => {
                // C.1: a floating argument goes in the next SIMD register…
                if nsrn < 8 {
                    args.push(Loc::Reg(PReg::fpr(nsrn), w));
                    nsrn += 1;
                } else {
                    // C.13/C.14: …otherwise on the stack, 8-byte aligned
                    nsaa = (nsaa + 7) & !7;
                    args.push(Loc::Stack(nsaa, w));
                    nsaa += 8;
                }
            }
            PTy::S(_) => {
                // C.9: an integral argument goes in the next general register…
                if ngrn < 8 {
                    args.push(Loc::Reg(PReg::gpr(ngrn), w));
                    ngrn += 1;
                } else {
                    nsaa = (nsaa + 7) & !7;
                    args.push(Loc::Stack(nsaa, w));
                    nsaa += 8;
                }
            }
            PTy::LDouble | PTy::Agg { .. } => todo!("R1.2: composite and binary128 arguments"),
        }
    }
    let ret = sig.ret.as_ref().map(|r| match r {
        PTy::S(t) if t.is_float() => Loc::Reg(PReg::fpr(0), width(r)),
        PTy::S(_) => Loc::Reg(PReg::gpr(0), width(r)),
        // §6.9: a composite larger than 16 bytes is returned indirectly via x8
        PTy::LDouble | PTy::Agg { .. } => todo!("R1.2: composite return"),
    });
    Assign {
        args,
        ret,
        stack_bytes: (nsaa + 15) & !15,
    }
}
