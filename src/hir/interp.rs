// ⟦hir⟧ — the executable reference semantics of HIR (SEMANTICS.md, REARCH §3.4).
//
// This is the *prover* half of Law 3: every HIR→HIR pass P ships the commuting
// square ⟦f⟧ = ⟦P f⟧, and this file is what evaluates both sides. It is not a
// debugging aid and not a fallback execution engine — it is the semantics, and
// the compiler's obligation is to agree with it.
//
// Σ = ⟨ values : ValueId → Bits, μ : flat little-endian byte memory (LP64, with
// the module's globals materialized), the call stack ⟩. A trap (C99 undefined
// behavior reached: division by zero, an out-of-range access, a missing extern)
// is ⊥, and a transform may REFINE ⊥ into anything — so a commuting square
// compares only runs where neither side traps.
use super::*;
use crate::ast::Ast;
use std::collections::HashMap;

pub type Bits = u64;

/// The interpreter's step budget. A non-terminating run is ⊥ for proof
/// purposes, and every commuting square compares only runs where neither side
/// traps — so the exact number is not a semantic constant, only a bound on how
/// long a battery may spend before declaring ⊥. 5·10^7 steps is roughly a
/// second of interpretation, which is far past anything a battery program does
/// (the heaviest today, fib(15), is ~10^5).
pub const STEP_BUDGET: u64 = 50_000_000;


pub use crate::mem::Trap;
use crate::mem::{FUNC_TAG, LABEL_TAG, Layout, Mem};

pub struct Machine<'a> {
    m: &'a Module,
    /// function name → index in `m.funcs`
    by_name: HashMap<&'a str, usize>,
    lay: Layout,
    mem: Mem,
    steps: u64,
}

pub fn new_machine<'a>(m: &'a Module, ast: &Ast) -> Machine<'a> {
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
        steps: 0,
    }
}

impl<'a> Machine<'a> {
    pub fn sym_addr(&self, s: &Sym) -> u64 {
        match s {
            Sym::Global(i) => self.lay.globals[*i as usize],
            Sym::Str(i) => self.lay.strs[*i as usize],
            // A function address is its index + a tag; only comparison and
            // indirect call use it, both of which go back through this map.
            Sym::Func(name) => match self.by_name.get(name.as_str()) {
                Some(i) => FUNC_TAG | *i as u64,
                None => 0,
            },
            Sym::Label(b) => LABEL_TAG | *b as u64,
        }
    }

    pub fn call(&mut self, name: &str, args: &[Bits]) -> Result<Option<Bits>, Trap> {
        let i = *self
            .by_name
            .get(name)
            .ok_or_else(|| Trap::NoSuchFunction(name.to_string()))?;
        self.call_index(i, args)
    }

    fn call_index(&mut self, fi: usize, args: &[Bits]) -> Result<Option<Bits>, Trap> {
        let f = &self.m.funcs[fi];
        let frame: u64 = f.slots.iter().map(|s| ((s.size + 15) & !15) as u64).sum();
        let base = self.mem.push_frame(frame)?;
        let mut slot_addr = Vec::with_capacity(f.slots.len());
        let mut at = base;
        for s in &f.slots {
            slot_addr.push(at);
            at += ((s.size + 15) & !15) as u64;
        }
        let r = self.run_body(fi, args, &slot_addr);
        self.mem.pop_frame(frame);
        r
    }

    fn run_body(
        &mut self,
        fi: usize,
        args: &[Bits],
        slot_addr: &[u64],
    ) -> Result<Option<Bits>, Trap> {
        let f = &self.m.funcs[fi];
        let mut vals: Vec<Bits> = vec![0; f.values.len()];
        for (i, vi) in f.values.iter().enumerate() {
            if let Def::FuncParam(k) = vi.def {
                vals[i] = args.get(k as usize).copied().unwrap_or(0);
            }
        }
        let mut b = f.entry;
        loop {
            let blk = &f.blocks[b as usize];
            for inst in &blk.insts {
                self.steps += 1;
                if self.steps > STEP_BUDGET {
                    return Err(Trap::OutOfSteps);
                }
                self.step(fi, inst, &mut vals, slot_addr)?;
            }
            let f = &self.m.funcs[fi];
            let blk = &f.blocks[b as usize];
            let get = |o: &Operand, vals: &Vec<Bits>| -> Bits {
                match o {
                    Operand::Val(v) => vals[*v as usize],
                    Operand::Imm(k) => *k as u64,
                    Operand::Fimm(k) => *k,
                }
            };
            let take = |t: &Target, vals: &mut Vec<Bits>| {
                let xs: Vec<Bits> = t.args.iter().map(|a| get(a, vals)).collect();
                for (p, x) in f.blocks[t.block as usize].params.iter().zip(xs) {
                    vals[*p as usize] = x;
                }
                t.block
            };
            b = match &blk.term {
                Term::Jmp(t) => take(t, &mut vals),
                Term::Br(c, x, y) => {
                    if get(c, &vals) as u32 != 0 {
                        take(x, &mut vals)
                    } else {
                        take(y, &mut vals)
                    }
                }
                Term::Switch(c, ty, arms, d) => {
                    let k = sext(get(c, &vals), *ty);
                    match arms.iter().find(|(v, _)| *v == k) {
                        Some((_, t)) => take(t, &mut vals),
                        None => take(d, &mut vals),
                    }
                }
                Term::Ret(v) => return Ok(v.as_ref().map(|o| get(o, &vals))),
                Term::Unreachable => return Err(Trap::Unreachable),
                Term::GotoPtr(o, _) => {
                    let a = get(o, &vals);
                    (a & 0xffff_ffff) as BlockId
                }
            };
        }
    }

    fn step(
        &mut self,
        fi: usize,
        inst: &Inst,
        vals: &mut Vec<Bits>,
        slot_addr: &[u64],
    ) -> Result<(), Trap> {
        let get = |o: &Operand, vals: &Vec<Bits>| -> Bits {
            match o {
                Operand::Val(v) => vals[*v as usize],
                Operand::Imm(k) => *k as u64,
                Operand::Fimm(k) => *k,
            }
        };
        match inst {
            Inst::Bin { dst, op, ty, a, b } => {
                let (x, y) = (get(a, vals), get(b, vals));
                vals[*dst as usize] = eval_bin(*op, *ty, x, y)?;
            }
            Inst::Un { dst, op, ty, a } => {
                let x = get(a, vals);
                vals[*dst as usize] = match op {
                    UnOp::Neg => mask(0u64.wrapping_sub(x), *ty),
                    UnOp::Not => mask(!x, *ty),
                    UnOp::FNeg => match ty {
                        Ty::F32 => (x as u32 ^ 0x8000_0000) as u64,
                        _ => x ^ 0x8000_0000_0000_0000,
                    },
                };
            }
            Inst::Cmp { dst, op, ty, a, b } => {
                let (x, y) = (get(a, vals), get(b, vals));
                vals[*dst as usize] = eval_cmp(*op, *ty, x, y) as u64;
            }
            Inst::Cvt {
                dst,
                op,
                from,
                to,
                a,
            } => {
                let x = get(a, vals);
                vals[*dst as usize] = eval_cvt(*op, *from, *to, x);
            }
            Inst::Load { dst, ty, addr, .. } => {
                let a = get(addr, vals);
                vals[*dst as usize] = self.mem.load(a, ty.bytes())?;
            }
            Inst::Store { ty, addr, val, .. } => {
                let (a, v) = (get(addr, vals), get(val, vals));
                self.mem.store(a, ty.bytes(), v)?;
            }
            Inst::SlotAddr { dst, slot, off } => {
                vals[*dst as usize] = (slot_addr[*slot as usize] as i64 + off) as u64;
            }
            Inst::SymAddr { dst, sym } => {
                vals[*dst as usize] = self.sym_addr(sym);
            }
            Inst::Select { dst, c, a, b, .. } => {
                let t = get(c, vals) as u32 != 0;
                vals[*dst as usize] = if t { get(a, vals) } else { get(b, vals) };
            }
            Inst::Alloca { dst, size, align } => {
                let n = get(size, vals).max(1);
                let a = self.mem.push_frame(n.max(*align as u64))?;
                vals[*dst as usize] = a;
            }
            Inst::MemCpy { dst, src, len } => {
                let (d, s) = (get(dst, vals), get(src, vals));
                for i in 0..*len {
                    let b = self.mem.load(s + i, 1)?;
                    self.mem.store(d + i, 1, b)?;
                }
            }
            Inst::MemSet { dst, byte, len } => {
                let (d, v) = (get(dst, vals), get(byte, vals) & 0xff);
                for i in 0..*len {
                    self.mem.store(d + i, 1, v)?;
                }
            }
            Inst::Call {
                dst,
                callee,
                args,
                sig,
            } => {
                let xs: Vec<Bits> = args.iter().map(|a| get(a, vals)).collect();
                let _ = sig;
                let r = match callee {
                    Callee::Direct(name) => match self.by_name.get(name.as_str()) {
                        Some(&i) => self.call_index(i, &xs)?,
                        None => self.builtin(name, &xs)?,
                    },
                    Callee::Indirect(o) => {
                        let a = get(o, vals);
                        let i = (a & 0xffff_ffff) as usize;
                        if a & FUNC_TAG == 0 || i >= self.m.funcs.len() {
                            return Err(Trap::BadAddress(a));
                        }
                        self.call_index(i, &xs)?
                    }
                };
                if let Some(d) = dst {
                    vals[*d as usize] = r.unwrap_or(0);
                }
            }
            Inst::Intrinsic { .. } => {
                let _ = fi;
                return Err(Trap::Unreachable); // R1: EXT intrinsics
            }
        }
        Ok(())
    }

    /// The externals a corpus function may reach. Anything else is ⊥ — the
    /// battery then simply does not use that function as a proof witness.
    fn builtin(&mut self, name: &str, a: &[Bits]) -> Result<Option<Bits>, Trap> {
        match name {
            "memcpy" | "__builtin_memcpy" => {
                for i in 0..a[2] {
                    let b = self.mem.load(a[1] + i, 1)?;
                    self.mem.store(a[0] + i, 1, b)?;
                }
                Ok(Some(a[0]))
            }
            "memset" | "__builtin_memset" => {
                for i in 0..a[2] {
                    self.mem.store(a[0] + i, 1, a[1] & 0xff)?;
                }
                Ok(Some(a[0]))
            }
            "strlen" => {
                let mut n = 0;
                while self.mem.load(a[0] + n, 1)? != 0 {
                    n += 1;
                }
                Ok(Some(n))
            }
            "abs" => Ok(Some((a[0] as i32).unsigned_abs() as u64)),
            "putchar" | "puts" | "printf" | "fprintf" => Ok(Some(0)),
            _ => Err(Trap::NoSuchFunction(name.to_string())),
        }
    }
}

// ── the scalar semantics (SEMANTICS.md §3) ─────────────────────────────────
/// Keep only the low `ty.bits()` bits, sign-extended into the 64-bit carrier.
/// Integers are held sign-extended so `Operand::Imm` and a loaded value of the
/// same C value compare equal.
pub fn mask(v: u64, ty: Ty) -> u64 {
    match ty {
        Ty::I8 => v as u8 as i8 as i64 as u64,
        Ty::I16 => v as u16 as i16 as i64 as u64,
        Ty::I32 => v as u32 as i32 as i64 as u64,
        _ => v,
    }
}
fn sext(v: u64, ty: Ty) -> i64 {
    mask(v, ty) as i64
}
fn zext(v: u64, ty: Ty) -> u64 {
    match ty {
        Ty::I8 => v & 0xff,
        Ty::I16 => v & 0xffff,
        Ty::I32 | Ty::F32 => v & 0xffff_ffff,
        _ => v,
    }
}
fn f32of(v: u64) -> f32 {
    f32::from_bits(v as u32)
}
fn f64of(v: u64) -> f64 {
    f64::from_bits(v)
}

fn eval_bin(op: BinOp, ty: Ty, x: u64, y: u64) -> Result<u64, Trap> {
    use BinOp::*;
    if ty.is_float() {
        let r = if ty == Ty::F32 {
            let (a, b) = (f32of(x), f32of(y));
            (match op {
                FAdd => a + b,
                FSub => a - b,
                FMul => a * b,
                FDiv => a / b,
                _ => unreachable!(),
            })
            .to_bits() as u64
        } else {
            let (a, b) = (f64of(x), f64of(y));
            (match op {
                FAdd => a + b,
                FSub => a - b,
                FMul => a * b,
                FDiv => a / b,
                _ => unreachable!(),
            })
            .to_bits()
        };
        return Ok(r);
    }
    let (sx, sy) = (sext(x, ty), sext(y, ty));
    let (ux, uy) = (zext(x, ty), zext(y, ty));
    let bits = ty.bits() as u64;
    let v = match op {
        Add => sx.wrapping_add(sy) as u64,
        Sub => sx.wrapping_sub(sy) as u64,
        Mul => sx.wrapping_mul(sy) as u64,
        SDiv => {
            if sy == 0 {
                return Err(Trap::DivZero);
            }
            sx.wrapping_div(sy) as u64
        }
        UDiv => {
            if uy == 0 {
                return Err(Trap::DivZero);
            }
            ux / uy
        }
        SRem => {
            if sy == 0 {
                return Err(Trap::DivZero);
            }
            sx.wrapping_rem(sy) as u64
        }
        URem => {
            if uy == 0 {
                return Err(Trap::DivZero);
            }
            ux % uy
        }
        And => x & y,
        Or => x | y,
        Xor => x ^ y,
        // C99 6.5.7p3: a shift count ≥ the width is undefined; the A64 shift
        // instructions take it modulo the width, and that is what we define.
        Shl => ux.wrapping_shl((uy % bits) as u32),
        LShr => ux.wrapping_shr((uy % bits) as u32),
        AShr => (sx >> (uy % bits)) as u64,
        FAdd | FSub | FMul | FDiv => unreachable!(),
    };
    Ok(mask(v, ty))
}

fn eval_cmp(op: CmpOp, ty: Ty, x: u64, y: u64) -> u32 {
    use CmpOp::*;
    let r = if op.is_float() {
        let (a, b) = if ty == Ty::F32 {
            (f32of(x) as f64, f32of(y) as f64)
        } else {
            (f64of(x), f64of(y))
        };
        match op {
            FOeq => a == b,
            FOne => a != b && !a.is_nan() && !b.is_nan(),
            FUne => a != b,
            FOlt => a < b,
            FOle => a <= b,
            FOgt => a > b,
            FOge => a >= b,
            FUno => a.is_nan() || b.is_nan(),
            _ => unreachable!(),
        }
    } else {
        let (sx, sy) = (sext(x, ty), sext(y, ty));
        let (ux, uy) = (zext(x, ty), zext(y, ty));
        match op {
            Eq => ux == uy,
            Ne => ux != uy,
            Slt => sx < sy,
            Sle => sx <= sy,
            Sgt => sx > sy,
            Sge => sx >= sy,
            Ult => ux < uy,
            Ule => ux <= uy,
            Ugt => ux > uy,
            Uge => ux >= uy,
            _ => unreachable!(),
        }
    };
    r as u32
}

fn eval_cvt(op: CvtOp, from: Ty, to: Ty, x: u64) -> u64 {
    use CvtOp::*;
    match op {
        Sext => mask(sext(x, from) as u64, to),
        Zext => mask(zext(x, from), to),
        Trunc => mask(x, to),
        Bitcast => match (from, to) {
            (Ty::F32, _) | (_, Ty::F32) => zext(x, Ty::I32),
            _ => x,
        },
        FpExt => f64::from(f32of(x)).to_bits(),
        FpTrunc => (f64of(x) as f32).to_bits() as u64,
        SiToFp => {
            let v = sext(x, from) as f64;
            if to == Ty::F32 { (v as f32).to_bits() as u64 } else { v.to_bits() }
        }
        UiToFp => {
            let v = zext(x, from) as f64;
            if to == Ty::F32 { (v as f32).to_bits() as u64 } else { v.to_bits() }
        }
        // C99 6.3.1.4: the value is truncated toward zero; out of range is
        // undefined, and A64 `fcvtzs` saturates — we define it that way.
        FpToSi => {
            let v = if from == Ty::F32 { f32of(x) as f64 } else { f64of(x) };
            mask(sat_i(v, to) as u64, to)
        }
        FpToUi => {
            let v = if from == Ty::F32 { f32of(x) as f64 } else { f64of(x) };
            mask(sat_u(v, to), to)
        }
    }
}

fn sat_i(v: f64, to: Ty) -> i64 {
    let bits = to.bits();
    let (lo, hi) = (-(2f64.powi(bits as i32 - 1)), 2f64.powi(bits as i32 - 1) - 1.0);
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
fn sat_u(v: f64, to: Ty) -> u64 {
    let hi = 2f64.powi(to.bits() as i32) - 1.0;
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= hi {
        hi as u64
    } else {
        v as u64
    }
}
