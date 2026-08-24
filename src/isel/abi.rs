// AAPCS64 parameter passing (IHI 0055 §6.4–6.8, the C.1–C.15 automaton).
// THEORY II-3 is the transcribed table; this file is the algorithm over it.
//
// The state is the spec's own three counters — NGRN (next general register),
// NSRN (next SIMD register), NSAA (next stacked-argument address) — so widening
// the rule set never rewrites the caller. Nothing here is a choice.
//
// TWO PLACES rule (Article E): the stack-argument offsets computed here must
// agree byte-for-byte with the parser's `va_off` walk (`parser.rs::setup_params`),
// because `__va_area__` is measured from the end of the named arguments. Edit
// both, then run `tests/abi.sh`.
use crate::hir::{PTy, Sig, Ty};
use crate::mir::{PReg, Width};

/// Where one argument (or the return value) is passed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Loc {
    /// a scalar in one register
    Reg(PReg, Width),
    /// a scalar at this byte offset into the outgoing-argument area (NSAA-relative)
    Stack(u32, Width),
    /// A composite in `n` consecutive registers of `esz` bytes each, starting at
    /// `first`: §6.8.2 (a ≤16-byte composite in x_n..x_{n+1}) and §5.9.5 (an HFA
    /// in v_n..v_{n+3}) are the same shape and are represented once.
    Regs {
        first: PReg,
        n: u32,
        esz: u32,
        size: u32,
    },
    /// a composite copied into the outgoing-argument area
    StackAgg { off: u32, size: u32 },
}

pub struct Assign {
    pub args: Vec<Loc>,
    /// where the result comes back; `None` for `void` and for the indirect case
    pub ret: Option<Loc>,
    /// §6.9: the result is a composite larger than 16 bytes — the CALLER passes
    /// the address of the destination in x8 and the callee writes through it.
    pub sret: bool,
    /// NSAA at the end: the size of the outgoing stack-argument area
    pub stack_bytes: u32,
    /// The three counters as they stand after the NAMED parameters — exactly
    /// what `va_start` has to record (AAPCS64 §6.4.2 + the va_list layout).
    pub ngrn: u32,
    pub nsrn: u32,
    pub nsaa: u32,
}

fn width(t: Ty) -> Width {
    match t {
        Ty::F32 => Width::S,
        Ty::F64 => Width::D,
        Ty::I64 => Width::W64,
        _ => Width::W32,
    }
}

fn up(v: u32, a: u32) -> u32 {
    (v + a - 1) & !(a - 1)
}

/// The C.1–C.15 walk.
pub fn classify(sig: &Sig) -> Assign {
    let (mut ngrn, mut nsrn, mut nsaa) = (0u32, 0u32, 0u32);
    let mut args = Vec::with_capacity(sig.params.len());
    for p in &sig.params {
        args.push(match p {
            // C.1: a floating argument goes in the next SIMD register…
            PTy::S(t) if t.is_float() => {
                if nsrn < 8 {
                    nsrn += 1;
                    Loc::Reg(PReg::fpr(nsrn as u8 - 1), width(*t))
                } else {
                    // C.13/C.14: …otherwise on the stack
                    let o = up(nsaa, 8);
                    nsaa = o + 8;
                    Loc::Stack(o, width(*t))
                }
            }
            // C.9: an integral argument goes in the next general register…
            PTy::S(t) => {
                if ngrn < 8 {
                    ngrn += 1;
                    Loc::Reg(PReg::gpr(ngrn as u8 - 1), width(*t))
                } else {
                    // C.14 + C.16: the NSAA is rounded to 8 and the argument
                    // occupies a full 8-byte slot regardless of its width.
                    let o = up(nsaa, 8);
                    nsaa = o + 8;
                    Loc::Stack(o, width(*t))
                }
            }
            // §5.1.2 / §6.4 C.2: binary128 is a Quad-precision Floating-point
            // type — one whole v register, or 16 stack bytes aligned to 16.
            PTy::LDouble => {
                if nsrn < 8 {
                    nsrn += 1;
                    Loc::Reg(PReg::fpr(nsrn as u8 - 1), Width::Q)
                } else {
                    nsrn = 8;
                    let o = up(nsaa, 16);
                    nsaa = o + 16;
                    Loc::Stack(o, Width::Q)
                }
            }
            // §5.9.5 HFA/HVA: 1–4 elements of one floating type, in v registers.
            PTy::Agg {
                size,
                align,
                hfa: Some((dbl, n)),
            } => {
                let esz = if *dbl { 8 } else { 4 };
                if nsrn + n <= 8 {
                    let first = PReg::fpr(nsrn as u8);
                    nsrn += n;
                    Loc::Regs {
                        first,
                        n: *n,
                        esz,
                        size: *size,
                    }
                } else {
                    // C.3: an HFA that does not fit LOCKS the remaining v registers
                    nsrn = 8;
                    let _ = align;
                    // Over-alignment is IGNORED for argument passing. AAPCS64
                    // C.13 says "the larger of 8 or the natural alignment", and
                    // gcc reads an `__attribute__((aligned(32)))` composite as
                    // still naturally 8-aligned: measured, it places such an
                    // argument at [sp, #0] and does NOT round NGRN to an even
                    // register for an `aligned(16)` one (torture pr92904). The
                    // callee's `va_arg` walk must round identically — and it
                    // rounds an ABSOLUTE address, which is only 16-byte aligned,
                    // so honouring 32 here would put the two sides on different
                    // bytes.
                    let o = up(nsaa, 8);
                    nsaa = o + size.div_ceil(8) * 8;
                    Loc::StackAgg { off: o, size: *size }
                }
            }
            // §6.8.2 B.4: a composite larger than 16 bytes is replaced by a
            // POINTER to a caller-made copy — an ordinary integral argument.
            // (The copy itself is made by the parser, which rewrites the call.)
            PTy::Agg { size, .. } if *size > 16 => {
                if ngrn < 8 {
                    ngrn += 1;
                    Loc::Reg(PReg::gpr(ngrn as u8 - 1), Width::W64)
                } else {
                    let o = up(nsaa, 8);
                    nsaa = o + 8;
                    Loc::Stack(o, Width::W64)
                }
            }
            // C.10: a composite of 16 bytes or fewer occupies ⌈size/8⌉ general
            // registers; C.11 locks NGRN when it does not fit.
            PTy::Agg { size, align, .. } => {
                let need = size.div_ceil(8).max(1);
                if ngrn + need <= 8 {
                    let first = PReg::gpr(ngrn as u8);
                    ngrn += need;
                    Loc::Regs {
                        first,
                        n: need,
                        esz: 8,
                        size: *size,
                    }
                } else {
                    ngrn = 8;
                    let _ = align;
                    let o = up(nsaa, 8);
                    nsaa = o + 8 * need;
                    Loc::StackAgg { off: o, size: *size }
                }
            }
        });
    }
    let mut sret = false;
    let ret = sig.ret.as_ref().and_then(|r| match r {
        PTy::S(t) if t.is_float() => Some(Loc::Reg(PReg::fpr(0), width(*t))),
        PTy::S(t) => Some(Loc::Reg(PReg::gpr(0), width(*t))),
        PTy::LDouble => Some(Loc::Reg(PReg::fpr(0), Width::Q)),
        // §6.9: an HFA comes back in v0.., any other composite ≤16B in x0..x1,
        // and anything larger indirectly through the x8 the caller supplied.
        PTy::Agg {
            size,
            hfa: Some((dbl, n)),
            ..
        } => Some(Loc::Regs {
            first: PReg::fpr(0),
            n: *n,
            esz: if *dbl { 8 } else { 4 },
            size: *size,
        }),
        PTy::Agg { size, .. } if *size > 16 => {
            sret = true;
            None
        }
        PTy::Agg { size, .. } => Some(Loc::Regs {
            first: PReg::gpr(0),
            n: size.div_ceil(8).max(1),
            esz: 8,
            size: *size,
        }),
    });
    Assign {
        args,
        ret,
        sret,
        stack_bytes: (nsaa + 15) & !15,
        ngrn,
        nsrn,
        nsaa: (nsaa + 7) & !7,
    }
}
