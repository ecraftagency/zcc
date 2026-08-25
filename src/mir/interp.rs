// ⟦mir⟧ — the executable semantics of the machine layer (REARCH.md §5.4).
//
// ONE interpreter for both lifecycle states. That is the point: `⟦hir⟧ =
// ⟦mir_v⟧` validates instruction selection, `⟦mir_v⟧ = ⟦mir_p⟧` validates
// register allocation (a renaming bisimulation), and `⟦mir_p⟧ = ⟦mir_final⟧`
// validates frame lowering and block layout — each an equality between two runs
// of THIS function, with no assembler, linker or hardware in the loop.
//
// Σ = ⟨ virtual registers (per call), the physical register file (persistent, so
// an AAPCS64 argument written by the caller is the same object the callee
// reads), NZCV, μ from `crate::mem`, sp ⟩.
use super::*;
use crate::ast::Ast;
use crate::mem::{FUNC_TAG, LABEL_TAG, Layout, Mem, Trap};
use std::collections::HashMap;

pub type Bits = u64;

/// The interpreter's step budget. A non-terminating run is ⊥ for proof
/// purposes, and every commuting square compares only runs where neither side
/// traps — so the exact number is not a semantic constant, only a bound on how
/// long a battery may spend before declaring ⊥. 5·10^7 steps is roughly a
/// second of interpretation, which is far past anything a battery program does
/// (the heaviest today, fib(15), is ~10^5).
pub const STEP_BUDGET: u64 = 50_000_000;


/// NZCV, packed as in the PSTATE encoding: N=8, Z=4, C=2, V=1.
const N: u64 = 8;
const Z: u64 = 4;
const C: u64 = 2;
const V: u64 = 1;

pub struct Machine<'a> {
    m: &'a MModule,
    by_name: HashMap<&'a str, usize>,
    lay: Layout,
    mem: Mem,
    gpr: [u64; 32],
    fpr: [u64; 32],
    /// The upper half of each v register. A `q` form (binary128 `long double`,
    /// the variadic save area) is the only thing that reads it; every other
    /// width leaves it alone, exactly as the hardware does not.
    fpr_hi: [u64; 32],
    nzcv: u64,
    steps: u64,
}

pub fn new_machine<'a>(m: &'a MModule, ast: &Ast) -> Machine<'a> {
    let (mem, lay) = crate::mem::build(ast);
    Machine {
        m,
        by_name: m
            .funcs
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.as_str(), i))
            .collect(),
        lay,
        mem,
        gpr: [0; 32],
        fpr: [0; 32],
        fpr_hi: [0; 32],
        nzcv: 0,
        steps: 0,
    }
}

/// Per-call state: the virtual register file and where this frame's stack
/// objects landed.
struct Frame {
    vals: Vec<u64>,
    /// the upper half of a `q`-width virtual register (see `Machine::fpr_hi`)
    hi: Vec<u64>,
    slot_addr: Vec<u64>,
    /// bytes taken by `StackAlloc` in this call, reclaimed when it returns —
    /// the interpreter's counterpart of the epilogue's `mov sp, x29`
    dyn_bytes: u64,
    outgoing: u64,
}

impl<'a> Machine<'a> {
    pub fn sym_addr(&self, s: &crate::hir::Sym) -> u64 {
        use crate::hir::Sym;
        match s {
            Sym::Global(i) | Sym::Tls(i) => self.lay.globals[*i as usize],
            Sym::Str(i) => self.lay.strs[*i as usize],
            Sym::Func(name) => match self.by_name.get(name.as_str()) {
                Some(i) => FUNC_TAG | *i as u64,
                None => 0,
            },
            Sym::Label(b) => LABEL_TAG | *b as u64,
        }
    }

    /// Call `name` with AAPCS64 scalar arguments: integers in x0–x7, floating
    /// values in v0–v7 (`fp` selects, positionally, which list an argument came
    /// from). R1 widens this to the full C.1–C.15 automaton.
    pub fn call(&mut self, name: &str, ints: &[Bits], flts: &[Bits]) -> Result<Bits, Trap> {
        for (i, v) in ints.iter().take(8).enumerate() {
            self.gpr[i] = *v;
        }
        for (i, v) in flts.iter().take(8).enumerate() {
            self.fpr[i] = *v;
        }
        let fi = *self
            .by_name
            .get(name)
            .ok_or_else(|| Trap::NoSuchFunction(name.to_string()))?;
        self.call_index(fi)?;
        Ok(self.gpr[0])
    }

    fn get(&self, fr: &Frame, r: Reg) -> u64 {
        match r {
            Reg::V(v) => fr.vals[v as usize],
            Reg::P(p) => match p.class {
                Class::Gpr => {
                    if p.num == 31 {
                        // xzr reads as zero; sp is read through `AddrMode`, never here
                        0
                    } else {
                        self.gpr[p.num as usize]
                    }
                }
                Class::Fpr => self.fpr[p.num as usize],
                Class::Flags => self.nzcv,
            },
        }
    }

    fn get_hi(&self, fr: &Frame, r: Reg) -> u64 {
        match r {
            Reg::V(v) => fr.hi[v as usize],
            Reg::P(p) => match p.class {
                Class::Fpr => self.fpr_hi[p.num as usize],
                _ => 0,
            },
        }
    }

    fn set_hi(&mut self, fr: &mut Frame, r: Reg, v: u64) {
        match r {
            Reg::V(x) => fr.hi[x as usize] = v,
            Reg::P(p) => {
                if p.class == Class::Fpr {
                    self.fpr_hi[p.num as usize] = v;
                }
            }
        }
    }

    /// Write an FP result. DDI 0487 C6.2: writing a v register in a SCALAR form
    /// (`s`/`d`) zeroes bits 127:64 — so a later `q` read of the same register
    /// must not see a stale upper half. Only the `q` forms carry one.
    fn set_fp(&mut self, fr: &mut Frame, r: Reg, v: u64, w: Width) {
        self.set(fr, r, v);
        if w != Width::Q {
            self.set_hi(fr, r, 0);
        }
    }

    fn set(&mut self, fr: &mut Frame, r: Reg, v: u64) {
        match r {
            Reg::V(x) => fr.vals[x as usize] = v,
            Reg::P(p) => match p.class {
                Class::Gpr => {
                    if p.num != 31 {
                        self.gpr[p.num as usize] = v;
                    }
                }
                Class::Fpr => self.fpr[p.num as usize] = v,
                Class::Flags => self.nzcv = v,
            },
        }
    }

    fn call_index(&mut self, fi: usize) -> Result<(), Trap> {
        let f = &self.m.funcs[fi];
        // A function that has not been through `pass/frame.rs` has no prologue,
        // so nothing in it preserves the callee-saved registers — yet the
        // allocator already relied on AAPCS64 §6.1.1 saying they are preserved.
        // Before frame lowering that promise is a CONTRACT the interpreter
        // honors; after it, real `Spill`/`Reload` instructions honor it. That is
        // exactly the obligation of frame lowering, and comparing the two runs
        // is its commuting square (`mir/pass/tests.rs`).
        let contract = !f.laid_out;
        let saved: Vec<(PReg, u64)> = if contract {
            (19..=28u8)
                .map(PReg::gpr)
                .chain((8..=15u8).map(PReg::fpr))
                .chain(std::iter::once(isa::LR))
                .map(|p| {
                    let v = match p.class {
                        Class::Fpr => self.fpr[p.num as usize],
                        _ => self.gpr[p.num as usize],
                    };
                    (p, v)
                })
                .collect()
        } else {
            Vec::new()
        };
        let f = &self.m.funcs[fi];
        // sp as the CALLER left it: the base of the incoming argument area
        // (AAPCS64's NSAA seen from this side of the `bl`).
        let entry_sp = self.mem.sp;
        // Before `frame` runs, each stack object is its own region; afterwards
        // `frame_size` is the whole frame and slots carry offsets into it. The
        // outgoing-argument area is reserved at the bottom in BOTH phases —
        // otherwise a pre-frame run would let an outgoing argument and slot 0
        // occupy the same bytes.
        let out = f.outgoing as u64;
        let (frame_bytes, per_slot): (u64, bool) = if f.laid_out {
            (f.frame_size as u64, false)
        } else {
            (
                out + f
                    .slots
                    .iter()
                    .filter(|s| s.kind != SlotKind::InArgs)
                    .map(|s| ((s.size + 15) & !15) as u64)
                    .sum::<u64>(),
                true,
            )
        };
        let base = self.mem.push_frame(frame_bytes)?;
        let mut slot_addr = Vec::with_capacity(f.slots.len());
        let mut at = base + if per_slot { out } else { 0 };
        for s in &f.slots {
            if s.kind == SlotKind::InArgs {
                slot_addr.push(entry_sp);
            } else if per_slot {
                slot_addr.push(at);
                at += ((s.size + 15) & !15) as u64;
            } else {
                slot_addr.push((base as i64 + s.off as i64) as u64);
            }
        }
        let mut fr = Frame {
            vals: vec![0; f.vregs.len()],
            hi: vec![0; f.vregs.len()],
            slot_addr,
            dyn_bytes: 0,
            outgoing: out,
        };
        let r = self.run(fi, &mut fr);
        self.mem.pop_frame(frame_bytes + fr.dyn_bytes);
        for (p, v) in saved {
            match p.class {
                Class::Fpr => self.fpr[p.num as usize] = v,
                _ => self.gpr[p.num as usize] = v,
            }
        }
        r
    }

    fn run(&mut self, fi: usize, fr: &mut Frame) -> Result<(), Trap> {
        let mut b = self.m.funcs[fi].entry;
        loop {
            let nin = self.m.funcs[fi].blocks[b as usize].insts.len();
            for i in 0..nin {
                self.steps += 1;
                if self.steps > STEP_BUDGET {
                    return Err(Trap::OutOfSteps);
                }
                let inst = self.m.funcs[fi].blocks[b as usize].insts[i].clone();
                self.step(&inst, fr)?;
            }
            let term = self.m.funcs[fi].blocks[b as usize].term.clone();
            let pick = |me: &mut Self, fr: &mut Frame, t: &MTarget| -> MBlockId {
                let xs: Vec<u64> = t.args.iter().map(|a| me.get(fr, *a)).collect();
                let ps = me.m.funcs[fi].blocks[t.block as usize].params.clone();
                for (p, x) in ps.iter().zip(xs) {
                    me.set(fr, *p, x);
                }
                t.block
            };
            b = match &term {
                MTerm::B(t) => pick(self, fr, t),
                MTerm::Bcc(cc, fl, t, e) => {
                    let n = self.get(fr, *fl);
                    if cond_true(*cc, n) {
                        pick(self, fr, t)
                    } else {
                        pick(self, fr, e)
                    }
                }
                MTerm::Cbz { w, reg, zero, t, f } => {
                    let x = trunc(self.get(fr, *reg), *w);
                    if (x == 0) == *zero {
                        pick(self, fr, t)
                    } else {
                        pick(self, fr, f)
                    }
                }
                MTerm::Tb {
                    w,
                    reg,
                    bit,
                    set,
                    t,
                    f,
                } => {
                    let x = trunc(self.get(fr, *reg), *w);
                    if ((x >> *bit) & 1 == 1) == *set {
                        pick(self, fr, t)
                    } else {
                        pick(self, fr, f)
                    }
                }
                MTerm::Switch {
                    idx,
                    table,
                    default,
                } => {
                    let k = self.get(fr, *idx) as usize;
                    match table.get(k) {
                        Some(t) => pick(self, fr, t),
                        None => pick(self, fr, default),
                    }
                }
                MTerm::Ret => return Ok(()),
                MTerm::BrReg(r, _) => (self.get(fr, *r) & 0xffff_ffff) as MBlockId,
                MTerm::Unreachable => return Err(Trap::Unreachable),
            };
        }
    }

    fn addr(&mut self, fr: &Frame, m: &AddrMode, size: u32) -> Result<(u64, Option<(Reg, u64)>), Trap> {
        Ok(match m {
            AddrMode::BaseImm { base, off } => (self.base_of(fr, *base).wrapping_add(*off as i64 as u64), None),
            AddrMode::BaseReg {
                base,
                idx,
                ext,
                shift,
            } => {
                let i = self.get(fr, *idx);
                let i = match ext {
                    Some(ExtKind::Sxtw) => i as u32 as i32 as i64 as u64,
                    Some(ExtKind::Uxtw) => i & 0xffff_ffff,
                    _ => i,
                };
                (
                    self.base_of(fr, *base).wrapping_add(i << *shift),
                    None,
                )
            }
            AddrMode::PreIdx { base, wb, off } => {
                let a = self.base_of(fr, *base).wrapping_add(*off as i64 as u64);
                (a, Some((*wb, a)))
            }
            AddrMode::PostIdx { base, wb, off } => {
                let a = self.base_of(fr, *base);
                (a, Some((*wb, a.wrapping_add(*off as i64 as u64))))
            }
            AddrMode::Slot { slot, off } => (
                (fr.slot_addr[*slot as usize] as i64 + *off as i64) as u64,
                None,
            ),
            // The outgoing-argument area rides at the CURRENT sp — that is what
            // makes it survive a `StackAlloc` (REARCH §5.2).
            AddrMode::SpArg { off } => (self.mem.sp + *off as u64, None),
            AddrMode::SymLo12 { base, .. } => {
                let _ = size;
                (self.get(fr, *base), None)
            }
        })
    }

    /// `sp` and `x29` are addresses, not ordinary register contents.
    fn base_of(&self, fr: &Frame, r: Reg) -> u64 {
        match r {
            // sp is an address, not register contents. x29 is NOT special: this
            // backend has no frame pointer (`pass/frame.rs`), every stack object
            // is addressed from sp.
            Reg::P(p) if p.class == Class::Gpr && p.num == 31 => self.mem.sp,
            _ => self.get(fr, r),
        }
    }

    fn step(&mut self, inst: &MInst, fr: &mut Frame) -> Result<(), Trap> {
        match inst {
            MInst::Alu {
                op,
                w,
                dst,
                a,
                b,
                flags,
            } => {
                let x = trunc(self.get(fr, *a), *w);
                let y = self.rhs(fr, b, *w);
                let (r, nz) = alu(*op, *w, x, y);
                self.set(fr, *dst, r);
                if let Some(fl) = flags {
                    self.set(fr, *fl, nz);
                }
            }
            MInst::Alu3 { op, w, dst, a, b, c } => {
                let (x, y, z) = (
                    trunc(self.get(fr, *a), *w),
                    trunc(self.get(fr, *b), *w),
                    trunc(self.get(fr, *c), *w),
                );
                let p = x.wrapping_mul(y);
                let r = match op {
                    Alu3Op::Madd => z.wrapping_add(p),
                    Alu3Op::Msub => z.wrapping_sub(p),
                };
                self.set(fr, *dst, trunc(r, *w));
            }
            MInst::Cmp { kind, w, a, b, flags } => {
                let x = trunc(self.get(fr, *a), *w);
                let y = self.rhs(fr, b, *w);
                let nz = match kind {
                    CmpKind::Cmp => alu(AluOp::Sub, *w, x, y).1,
                    CmpKind::Cmn => alu(AluOp::Add, *w, x, y).1,
                    CmpKind::Tst => alu(AluOp::And, *w, x, y).1,
                };
                self.set(fr, *flags, nz);
            }
            MInst::MovImm { w, dst, imm } => {
                let v = trunc(*imm as u64, *w);
                self.set(fr, *dst, v);
            }
            MInst::Ext { op, w, dst, src } => {
                let x = self.get(fr, *src);
                let v = match op {
                    ExtOp::Sxtb => x as u8 as i8 as i64 as u64,
                    ExtOp::Sxth => x as u16 as i16 as i64 as u64,
                    ExtOp::Sxtw => x as u32 as i32 as i64 as u64,
                    ExtOp::Uxtb => x & 0xff,
                    ExtOp::Uxth => x & 0xffff,
                };
                self.set(fr, *dst, trunc(v, *w));
            }
            MInst::Load { op, dst, mem, .. } => {
                let (a, wb) = self.addr(fr, mem, op.bytes())?;
                if *op == MemOp::Q {
                    let (lo, hi) = (self.mem.load(a, 8)?, self.mem.load(a + 8, 8)?);
                    self.set(fr, *dst, lo);
                    self.set_hi(fr, *dst, hi);
                    if let Some((r, x)) = wb {
                        self.set(fr, r, x);
                    }
                    return Ok(());
                }
                let raw = self.mem.load(a, op.bytes())?;
                let v = match op {
                    // the `w` forms extend within 32 bits and zero above
                    MemOp::SB => raw as u8 as i8 as i32 as u32 as u64,
                    MemOp::SBX => raw as u8 as i8 as i64 as u64,
                    MemOp::SH => raw as u16 as i16 as i32 as u32 as u64,
                    MemOp::SHX => raw as u16 as i16 as i64 as u64,
                    MemOp::SW => raw as u32 as i32 as i64 as u64,
                    _ => raw,
                };
                if op.class() == Class::Fpr {
                    self.set_hi(fr, *dst, 0); // `ldr s`/`ldr d` zero bits 127:64
                }
                self.set(fr, *dst, v);
                if let Some((r, x)) = wb {
                    self.set(fr, r, x);
                }
            }
            MInst::Store { op, src, mem, .. } => {
                let (a, wb) = self.addr(fr, mem, op.bytes())?;
                let v = self.get(fr, *src);
                if *op == MemOp::Q {
                    let hi = self.get_hi(fr, *src);
                    self.mem.store(a, 8, v)?;
                    self.mem.store(a + 8, 8, hi)?;
                } else {
                    self.mem.store(a, op.bytes(), v)?;
                }
                if let Some((r, x)) = wb {
                    self.set(fr, r, x);
                }
            }
            MInst::Bfx { signed, w, dst, src, lsb, width } => {
                let v = self.get(fr, *src) >> *lsb as u32;
                let m = if *width >= 64 { u64::MAX } else { (1u64 << *width) - 1 };
                let x = v & m;
                let x = if *signed && *width < 64 && (x >> (*width - 1)) & 1 == 1 {
                    x | !m
                } else {
                    x
                };
                let x = if *w == Width::W32 { x & 0xffff_ffff } else { x };
                self.set(fr, *dst, x);
            }
            // `ldp`/`stp`: the second register sits one element past the first.
            // The `q` form moves SIXTEEN bytes per register, which no single
            // `u64` holds, so it goes half at a time — the same shape `Load`,
            // `Store`, `Spill` and `Reload` already use for `Width::Q`.
            MInst::Pair { w, load, a, b, mem } => {
                let n = w.bytes();
                let (base, wb) = self.addr(fr, mem, n)?;
                let step = n as u64;
                let (half, q) = if *w == Width::Q { (8, true) } else { (n, false) };
                if *load {
                    let (x, y) = (self.mem.load(base, half)?, self.mem.load(base + step, half)?);
                    self.set(fr, *a, x);
                    self.set(fr, *b, y);
                    if q {
                        let (xh, yh) = (
                            self.mem.load(base + 8, 8)?,
                            self.mem.load(base + step + 8, 8)?,
                        );
                        self.set_hi(fr, *a, xh);
                        self.set_hi(fr, *b, yh);
                    }
                } else {
                    let (x, y) = (self.get(fr, *a), self.get(fr, *b));
                    self.mem.store(base, half, x)?;
                    self.mem.store(base + step, half, y)?;
                    if q {
                        let (xh, yh) = (self.get_hi(fr, *a), self.get_hi(fr, *b));
                        self.mem.store(base + 8, 8, xh)?;
                        self.mem.store(base + step + 8, 8, yh)?;
                    }
                }
                if let Some((r, x)) = wb {
                    self.set(fr, r, x);
                }
            }
            MInst::Adrp { dst, sym, .. } => {
                let a = self.sym_addr(sym);
                self.set(fr, *dst, a);
            }
            MInst::AddLo12 { dst, base, .. } => {
                let a = self.get(fr, *base);
                self.set(fr, *dst, a);
            }
            MInst::CSel {
                op,
                w,
                dst,
                a,
                b,
                cc,
                flags,
            } => {
                let n = self.get(fr, *flags);
                let (x, y) = (trunc(self.get(fr, *a), *w), trunc(self.get(fr, *b), *w));
                let v = if cond_true(*cc, n) {
                    x
                } else {
                    match op {
                        CSelOp::Csel => y,
                        CSelOp::Csinc => trunc(y.wrapping_add(1), *w),
                        CSelOp::Csinv => trunc(!y, *w),
                        CSelOp::Csneg => trunc(0u64.wrapping_sub(y), *w),
                    }
                };
                self.set(fr, *dst, v);
            }
            MInst::CSet { w, dst, cc, flags } => {
                let n = self.get(fr, *flags);
                let v = cond_true(*cc, n) as u64;
                self.set(fr, *dst, trunc(v, *w));
            }
            MInst::FpAlu { op, w, dst, a, b } => {
                let (x, y) = (self.get(fr, *a), self.get(fr, *b));
                let v = if *w == Width::S {
                    let (p, q) = (f32::from_bits(x as u32), f32::from_bits(y as u32));
                    (match op {
                        FpOp::Fadd => p + q,
                        FpOp::Fsub => p - q,
                        FpOp::Fmul => p * q,
                        FpOp::Fdiv => p / q,
                    })
                    .to_bits() as u64
                } else {
                    let (p, q) = (f64::from_bits(x), f64::from_bits(y));
                    (match op {
                        FpOp::Fadd => p + q,
                        FpOp::Fsub => p - q,
                        FpOp::Fmul => p * q,
                        FpOp::Fdiv => p / q,
                    })
                    .to_bits()
                };
                self.set_fp(fr, *dst, v, *w);
            }
            MInst::FpUn { op, w, dst, src, sw } => {
                let x = self.get(fr, *src);
                let v = match op {
                    FpUnOp::Fcvt => {
                        if *sw == Width::S {
                            (f32::from_bits(x as u32) as f64).to_bits()
                        } else {
                            (f64::from_bits(x) as f32).to_bits() as u64
                        }
                    }
                    _ if *w == Width::S => {
                        let p = f32::from_bits(x as u32);
                        (match op {
                            FpUnOp::Fneg => -p,
                            FpUnOp::Fabs => p.abs(),
                            FpUnOp::Fsqrt => p.sqrt(),
                            FpUnOp::Fcvt => unreachable!(),
                        })
                        .to_bits() as u64
                    }
                    _ => {
                        let p = f64::from_bits(x);
                        (match op {
                            FpUnOp::Fneg => -p,
                            FpUnOp::Fabs => p.abs(),
                            FpUnOp::Fsqrt => p.sqrt(),
                            FpUnOp::Fcvt => unreachable!(),
                        })
                        .to_bits()
                    }
                };
                self.set_fp(fr, *dst, v, *w);
            }
            MInst::FpCmp {
                w,
                a,
                b,
                zero,
                flags,
            } => {
                let x = self.get(fr, *a);
                let y = if *zero { 0 } else { self.get(fr, *b) };
                let (p, q) = if *w == Width::S {
                    (
                        f32::from_bits(x as u32) as f64,
                        f32::from_bits(y as u32) as f64,
                    )
                } else {
                    (f64::from_bits(x), f64::from_bits(y))
                };
                // DDI 0487 C6: unordered sets C and V, clears N and Z.
                let nz = if p.is_nan() || q.is_nan() {
                    C | V
                } else if p == q {
                    Z | C
                } else if p < q {
                    N
                } else {
                    C
                };
                self.set(fr, *flags, nz);
            }
            MInst::FpCvt {
                op,
                dw,
                sw,
                dst,
                src,
            } => {
                let x = self.get(fr, *src);
                let v = match op {
                    CvtOp::Scvtf => {
                        let i = if *sw == Width::W32 {
                            x as u32 as i32 as f64
                        } else {
                            x as i64 as f64
                        };
                        if *dw == Width::S {
                            (i as f32).to_bits() as u64
                        } else {
                            i.to_bits()
                        }
                    }
                    CvtOp::Ucvtf => {
                        let i = if *sw == Width::W32 {
                            (x as u32) as f64
                        } else {
                            x as f64
                        };
                        if *dw == Width::S {
                            (i as f32).to_bits() as u64
                        } else {
                            i.to_bits()
                        }
                    }
                    CvtOp::Fcvtzs => {
                        let d = if *sw == Width::S {
                            f32::from_bits(x as u32) as f64
                        } else {
                            f64::from_bits(x)
                        };
                        trunc(sat_i(d, *dw) as u64, *dw)
                    }
                    CvtOp::Fcvtzu => {
                        let d = if *sw == Width::S {
                            f32::from_bits(x as u32) as f64
                        } else {
                            f64::from_bits(x)
                        };
                        trunc(sat_u(d, *dw), *dw)
                    }
                };
                self.set_fp(fr, *dst, v, *dw);
            }
            MInst::FMov { dw, sw, dst, src } => {
                let x = self.get(fr, *src);
                let v = match (sw, dw) {
                    (Width::S, Width::W32) | (Width::W32, Width::S) => x & 0xffff_ffff,
                    _ => x,
                };
                // the 128-bit form is the vector move `mov Vd.16b, Vn.16b`, the
                // only one that carries the upper half
                if *dw == Width::Q {
                    let h = self.get_hi(fr, *src);
                    self.set(fr, *dst, v);
                    self.set_hi(fr, *dst, h);
                } else {
                    self.set_fp(fr, *dst, v, *dw);
                }
            }
            MInst::Call {
                callee,
                uses,
                defs,
                stack_bytes,
                ..
            } => {
                // The fixed constraints ARE the calling convention: move each
                // argument into the register the ABI named, then run the callee
                // against the same physical file.
                let vals: Vec<u64> = uses.iter().map(|(r, _)| self.get(fr, *r)).collect();
                for ((_, p), v) in uses.iter().zip(vals) {
                    self.set(fr, Reg::P(*p), v);
                }
                let _ = stack_bytes;
                match callee {
                    CallTarget::Direct(name) => match self.by_name.get(name.as_str()) {
                        Some(&i) => self.call_index(i)?,
                        None => self.builtin(&name.clone())?,
                    },
                    CallTarget::Indirect(r) => {
                        let a = self.get(fr, *r);
                        let i = (a & 0xffff_ffff) as usize;
                        if a & FUNC_TAG == 0 || i >= self.m.funcs.len() {
                            return Err(Trap::BadAddress(a));
                        }
                        self.call_index(i)?;
                    }
                }
                let outs: Vec<u64> = defs.iter().map(|(_, p)| self.get(fr, Reg::P(*p))).collect();
                for ((r, _), v) in defs.iter().zip(outs) {
                    self.set(fr, *r, v);
                }
            }
            MInst::Copy { dst, src, w } => {
                let v = trunc(self.get(fr, *src), *w);
                let h = self.get_hi(fr, *src);
                self.set(fr, *dst, v);
                if w.class() == Class::Fpr {
                    self.set_hi(fr, *dst, if *w == Width::Q { h } else { 0 });
                }
            }
            MInst::ParallelCopy(pairs) => {
                let vs: Vec<(u64, u64)> = pairs
                    .iter()
                    .map(|(_, s, w)| (trunc(self.get(fr, *s), *w), self.get_hi(fr, *s)))
                    .collect();
                for ((d, _, w), (v, h)) in pairs.iter().zip(vs) {
                    self.set(fr, *d, v);
                    if w.class() == Class::Fpr {
                        self.set_hi(fr, *d, if *w == Width::Q { h } else { 0 });
                    }
                }
            }
            MInst::Spill { slot, src, w } => {
                let a = fr.slot_addr[*slot as usize];
                let v = self.get(fr, *src);
                if *w == Width::Q {
                    let h = self.get_hi(fr, *src);
                    self.mem.store(a, 8, v)?;
                    self.mem.store(a + 8, 8, h)?;
                } else {
                    self.mem.store(a, w.bytes(), v)?;
                }
            }
            MInst::Reload { slot, dst, w } => {
                let a = fr.slot_addr[*slot as usize];
                if *w == Width::Q {
                    let (lo, hi) = (self.mem.load(a, 8)?, self.mem.load(a + 8, 8)?);
                    self.set(fr, *dst, lo);
                    self.set_hi(fr, *dst, hi);
                } else {
                    let v = self.mem.load(a, w.bytes())?;
                    self.set_fp(fr, *dst, v, *w);
                }
            }
            // B2.9: single-threaded, an exclusive pair is an ordinary
            // load/store and the store always succeeds — which is exactly the
            // behaviour ⟦·⟧ must model, since the interpreter has one thread.
            MInst::LdAxr { w, dst, addr } => {
                let a = self.get(fr, *addr);
                let v = self.mem.load(a, w.bytes())?;
                self.set(fr, *dst, v);
            }
            MInst::StlXr {
                w,
                status,
                src,
                addr,
            } => {
                let (a, v) = (self.get(fr, *addr), self.get(fr, *src));
                self.mem.store(a, w.bytes(), v)?;
                self.set(fr, *status, 0);
            }
            MInst::Stlr { w, src, addr } => {
                let (a, v) = (self.get(fr, *addr), self.get(fr, *src));
                self.mem.store(a, w.bytes(), v)?;
            }
            MInst::Dmb => {}
            // ⟦·⟧ has one thread, so the thread pointer is the origin and the
            // two halves of the tprel pair simply re-add up to the object's
            // address — the same value the linker's relocation produces.
            MInst::Mrs { dst } => self.set(fr, *dst, 0),
            MInst::AddTprel {
                dst,
                base,
                sym,
                hi,
            } => {
                let a = self.sym_addr(sym);
                let part = if *hi { a & !0xfff } else { a & 0xfff };
                let b = self.get(fr, *base);
                self.set(fr, *dst, b.wrapping_add(part));
            }
            // an asm template is opaque: no semantics to give it
            MInst::Asm { .. } => return Err(Trap::Unreachable),
            MInst::SlotAddr { dst, slot, off } => {
                let a = (fr.slot_addr[*slot as usize] as i64 + *off as i64) as u64;
                self.set(fr, *dst, a);
            }
            MInst::SpAddr { dst, off } => {
                let a = self.mem.sp + *off as u64;
                self.set(fr, *dst, a);
            }
            // C99 6.7.5.2: sp drops by the rounded byte count and the value is
            // the base of the new block. ⟦·⟧ keeps the count so the return can
            // reclaim it, which is what the epilogue's `mov sp, x29` does.
            MInst::StackAlloc { dst, size } => {
                let n = (self.get(fr, *size) + 15) & !15;
                let a = self.mem.push_frame(n)?;
                fr.dyn_bytes += n;
                // the new object starts ABOVE the outgoing-argument area, which
                // has moved down with sp and must stay reachable
                self.set(fr, *dst, a + fr.outgoing);
            }
        }
        Ok(())
    }

    fn rhs(&self, fr: &Frame, b: &Rhs, w: Width) -> u64 {
        match b {
            Rhs::Reg(r) => trunc(self.get(fr, *r), w),
            Rhs::Imm(k) => trunc(*k as u64, w),
            Rhs::Shifted(r, kind, amt) => {
                let x = trunc(self.get(fr, *r), w);
                let bits = if w.is64() { 64 } else { 32 };
                let a = *amt as u32 % bits;
                trunc(
                    match kind {
                        ShiftKind::Lsl => x << a,
                        ShiftKind::Lsr => x >> a,
                        ShiftKind::Asr => (sign(x, w) >> a) as u64,
                        ShiftKind::Ror => x.rotate_right(a),
                    },
                    w,
                )
            }
            Rhs::Extended(r, kind, amt) => {
                let x = self.get(fr, *r);
                let e = match kind {
                    ExtKind::Uxtb => x & 0xff,
                    ExtKind::Uxth => x & 0xffff,
                    ExtKind::Uxtw => x & 0xffff_ffff,
                    ExtKind::Uxtx => x,
                    ExtKind::Sxtb => x as u8 as i8 as i64 as u64,
                    ExtKind::Sxth => x as u16 as i16 as i64 as u64,
                    ExtKind::Sxtw => x as u32 as i32 as i64 as u64,
                    ExtKind::Sxtx => x,
                };
                trunc(e << *amt, w)
            }
        }
    }

    fn builtin(&mut self, name: &str) -> Result<(), Trap> {
        let a = self.gpr;
        match name {
            "memcpy" | "__builtin_memcpy" => {
                for i in 0..a[2] {
                    let b = self.mem.load(a[1] + i, 1)?;
                    self.mem.store(a[0] + i, 1, b)?;
                }
                self.gpr[0] = a[0];
            }
            "memset" | "__builtin_memset" => {
                for i in 0..a[2] {
                    self.mem.store(a[0] + i, 1, a[1] & 0xff)?;
                }
                self.gpr[0] = a[0];
            }
            "strlen" => {
                let mut n = 0;
                while self.mem.load(a[0] + n, 1)? != 0 {
                    n += 1;
                }
                self.gpr[0] = n;
            }
            "abs" => self.gpr[0] = (a[0] as i32).unsigned_abs() as u64,
            // THEORY II-2: the libgcc soft-float pair zcc's long double rides
            // on. Argument and result both live in q0/d0, so there is nothing
            // to marshal — only the format change (see `hir::interp::f64_to_f128`).
            "__extenddftf2" => {
                let (lo, hi) = crate::hir::interp::f64_to_f128(self.fpr[0]);
                self.fpr[0] = lo;
                self.fpr_hi[0] = hi;
            }
            "__trunctfdf2" => {
                self.fpr[0] = crate::hir::interp::f128_to_f64(self.fpr[0], self.fpr_hi[0]);
            }
            "putchar" | "puts" | "printf" | "fprintf" => self.gpr[0] = 0,
            _ => return Err(Trap::NoSuchFunction(name.to_string())),
        }
        Ok(())
    }
}

/// A64 `w`-form results are zero-extended into the 64-bit register — the machine
/// truth, and the one place ⟦mir⟧ deliberately differs from ⟦hir⟧'s
/// sign-extended carrier.
fn trunc(v: u64, w: Width) -> u64 {
    match w {
        Width::W32 | Width::S => v & 0xffff_ffff,
        _ => v,
    }
}
fn sign(v: u64, w: Width) -> i64 {
    if w.is64() {
        v as i64
    } else {
        v as u32 as i32 as i64
    }
}

/// The ALU, returning the result together with the NZCV it would set in its
/// flag-setting form (DDI 0487 C6.2 `AddWithCarry`).
fn alu(op: AluOp, w: Width, x: u64, y: u64) -> (u64, u64) {
    let bits = if w.is64() { 64 } else { 32 };
    let m = if w.is64() { u64::MAX } else { 0xffff_ffff };
    let (r, carry, ovf) = match op {
        AluOp::Add => {
            let s = x.wrapping_add(y) & m;
            let c = (x as u128 + y as u128) >> bits != 0;
            let v = ((x ^ s) & (y ^ s)) >> (bits - 1) & 1 != 0;
            (s, c, v)
        }
        AluOp::Sub => {
            let ny = (!y) & m;
            let s = x.wrapping_add(ny).wrapping_add(1) & m;
            let c = (x as u128 + ny as u128 + 1) >> bits != 0;
            let v = ((x ^ y) & (x ^ s)) >> (bits - 1) & 1 != 0;
            (s, c, v)
        }
        // DDI 0487 C6.2.199/244: the high half of the full 128-bit product,
        // defined only in the 64-bit form.
        AluOp::SMulH => (
            (((x as i64 as i128).wrapping_mul(y as i64 as i128)) >> 64) as u64,
            false,
            false,
        ),
        AluOp::UMulH => (
            (((x as u128).wrapping_mul(y as u128)) >> 64) as u64,
            false,
            false,
        ),
        AluOp::And => (x & y, false, false),
        AluOp::Orr => (x | y, false, false),
        AluOp::Eor => (x ^ y, false, false),
        AluOp::Bic => (x & !y & m, false, false),
        AluOp::Orn => (x | (!y & m), false, false),
        AluOp::Eon => (x ^ (!y & m), false, false),
        AluOp::Lsl => ((x << (y % bits as u64)) & m, false, false),
        AluOp::Lsr => ({
            let s = if w.is64() { x } else { x & 0xffff_ffff };
            s >> (y % bits as u64)
        }, false, false),
        AluOp::Asr => ((sign(x, w) >> (y % bits as u64)) as u64 & m, false, false),
        AluOp::Mul => (x.wrapping_mul(y) & m, false, false),
        AluOp::SDiv => {
            // A64 `sdiv` by zero yields ZERO (it does not trap); C99 leaves the
            // program undefined, so refining ⊥ to 0 is legal.
            let (a, b) = (sign(x, w), sign(y, w));
            let q = if b == 0 { 0 } else { a.wrapping_div(b) };
            (q as u64 & m, false, false)
        }
        AluOp::UDiv => {
            let (a, b) = (x & m, y & m);
            (if b == 0 { 0 } else { a / b }, false, false)
        }
    };
    let n = if (r >> (bits - 1)) & 1 != 0 { N } else { 0 };
    let z = if r & m == 0 { Z } else { 0 };
    (r, n | z | if carry { C } else { 0 } | if ovf { V } else { 0 })
}

fn cond_true(cc: CC, f: u64) -> bool {
    let (n, z, c, v) = (f & N != 0, f & Z != 0, f & C != 0, f & V != 0);
    match cc {
        CC::Eq => z,
        CC::Ne => !z,
        CC::Hs => c,
        CC::Lo => !c,
        CC::Mi => n,
        CC::Pl => !n,
        CC::Vs => v,
        CC::Vc => !v,
        CC::Hi => c && !z,
        CC::Ls => !c || z,
        CC::Ge => n == v,
        CC::Lt => n != v,
        CC::Gt => !z && n == v,
        CC::Le => z || n != v,
    }
}

fn sat_i(v: f64, w: Width) -> i64 {
    let bits = if w.is64() { 64 } else { 32 };
    let (lo, hi) = (-(2f64.powi(bits - 1)), 2f64.powi(bits - 1) - 1.0);
    if v.is_nan() {
        0
    } else if v <= lo {
        lo as i64
    } else if v >= hi {
        hi as i64
    } else {
        v as i64
    }
}
fn sat_u(v: f64, w: Width) -> u64 {
    let bits = if w.is64() { 64 } else { 32 };
    let hi = 2f64.powi(bits) - 1.0;
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= hi {
        hi as u64
    } else {
        v as u64
    }
}
