//! IR -> AArch64 emitter: the per-Inst lowering + AAPCS64 call marshalling (ASlot).
//! Side-I algorithm (the lowering theorems); consumes phi-free IR, simulates each
//! Inst through x0/x1 into the temp's home/slot. Methods are pub(super) so the
//! emit_ir spine and the value-contract impl (lower.rs) can cross-call them.
use super::{index_live_at, Cg, ExtFold, ParamLoc};
use super::encoding::{
    add_sub_imm12, fp_phys, inv_cond, is_logical_imm, rel_cond, sym, xr, EPILOGUE, FP_BUDGET,
    GP_BUDGET, GP_BUDGET_WIDE,
};
use crate::ast::{SyncOp, Ty, TypeId};
use crate::ir::{self, Callee, Inst, IrFunc, Op, Place, Term, Tmp, Un, Val};
use crate::opt::{each_use_mut, each_use_term_mut};
use std::fmt::Write;


// ═══════════════════════════════════════════════════════════════════════════
// IR → asm PATH — the SOLE path (the AST-walk has been removed). Naive stack-slot
// model: each temp gets an 8B slot below the frame (x29 − (frame+8+i*8)); load
// operands into x0/x1, compute, and str the result back to the slot. Reuses the
// value-contract methods (load/store/cast_op/ext/imm/lea_local). Every C99 construct
// lowers to a typed Inst; there is no Opaque bridge — no node re-emits an AST subtree.
// ═══════════════════════════════════════════════════════════════════════════
// AAPCS slot for ir_call_abi. G=x-reg, F=v-reg float (4B needs fcvt), S=scalar→stack,
// St=struct→GPR (2 regs?), StS=struct→stack, H=HFA→v-reg, Q=ldouble q.
#[derive(Clone, Copy)]
enum ASlot {
    G(u32),
    F(u32, bool),
    S(u32, u32, bool),
    St(u32, bool),
    StS(u32, u32),
    H(u32, u32, bool),
    Q(u32),
}

impl<'a> Cg<'a> {
    pub(super) fn ir_toff(&self, i: Tmp) -> u32 {
        self.ir_tbase + 8 + self.spill_off[i as usize]
    }
    // GP color → physical register, per the ACTIVE budget for this function (§3). WIDE opens
    // 6 caller-saved homes x10–x15 (colors 0..6) ahead of the callee-saved x19–x28; NARROW is
    // the callee-only file. `gp_ncaller()` reports the split so csave / verify agree.
    pub(super) fn gpp(&self, idx: u32) -> u32 {
        if self.gp_wide {
            // caller colors 0–7 → x0–x7 (the ARGUMENT/result registers — enables value-placement
            // targeting: an arg temp homed at x{i} makes its marshal `mov x{i},x{i}` vanish, a
            // call result homed at x0 makes its capture `mov home,x0` vanish); callee 8–17 →
            // x19–x28. The funnel scratch that used to live in x0–x5 now lives in x9–x14 (disjoint).
            if idx < GP_BUDGET_WIDE.ncaller { idx } else { 19 + (idx - GP_BUDGET_WIDE.ncaller) }
        } else {
            19 + idx
        }
    }
    pub(super) fn gp_ncaller(&self) -> u32 {
        if self.gp_wide { GP_BUDGET_WIDE.ncaller } else { GP_BUDGET.ncaller }
    }
    // Stage 5b — a temp's home is a physical register (Chaitin color) or a spill slot.
    // `reg` is always a 64-bit GPR (verified: every call site passes an x-form); an
    // FP-homed temp holds the f64 bit pattern (SEMANTICS §1), moved via `fmov` GPR↔d-reg.
    pub(super) fn tmp_load(&mut self, i: Tmp, reg: &str) {
        match self.talloc.get(i as usize).copied().flatten() {
            Some((true, idx)) => _ = writeln!(self.s, "\tfmov {reg}, d{}", fp_phys(idx)),
            Some((false, idx)) => _ = writeln!(self.s, "\tmov {reg}, x{}", self.gpp(idx)),
            None => {
                let off = self.ir_toff(i);
                if let Some(pos) = self.sp_slot(off) {
                    _ = writeln!(self.s, "\tldr {reg}, [sp, #{pos}]");
                } else if off <= 256 {
                    // Addressing-model fix (§4/§5): x29 is a fixed frame pointer (`mov x29,sp`,
                    // never reassigned) so `[x29,#-off]` is the identical effective address as
                    // `sub x9,x29,#off; ldr [x9]` — one `ldur` (imm9 unscaled, −256..255)
                    // replaces the two-instruction lea form and clobbers no x9. Valid even
                    // when sp is displaced (marshalling/VLA), since x29 does not move.
                    _ = writeln!(self.s, "\tldur {reg}, [x29, #-{off}]");
                } else {
                    self.lea_local("x9", off);
                    _ = writeln!(self.s, "\tldr {reg}, [x9]");
                }
            }
        }
    }
    pub(super) fn tmp_store(&mut self, i: Tmp, reg: &str) {
        match self.talloc.get(i as usize).copied().flatten() {
            Some((true, idx)) => _ = writeln!(self.s, "\tfmov d{}, {reg}", fp_phys(idx)),
            Some((false, idx)) => _ = writeln!(self.s, "\tmov x{}, {reg}", self.gpp(idx)),
            None => {
                let off = self.ir_toff(i);
                if let Some(pos) = self.sp_slot(off) {
                    _ = writeln!(self.s, "\tstr {reg}, [sp, #{pos}]");
                } else if off <= 256 {
                    // See tmp_load: `stur {reg},[x29,#-off]` = `sub x9,x29,#off; str [x9]`.
                    _ = writeln!(self.s, "\tstur {reg}, [x29, #-{off}]");
                } else {
                    self.lea_local("x9", off);
                    _ = writeln!(self.s, "\tstr {reg}, [x9]");
                }
            }
        }
    }
    // Save (`store=true`) or restore the callee-saved registers used by this function
    // into/from the frame-bottom slab. x29-relative (stable under VLA sp movement); the
    // slab occupies the lowest `ir_tspill` bytes, so `reset_sp_base` keeps it above sp.
    pub(super) fn save_callee(&mut self, store: bool) {
        let (gp, fp) = (self.csave_gp.clone(), self.csave_fp.clone());
        if gp.is_empty() && fp.is_empty() {
            return;
        }
        self.lea_local("x9", self.ir_tbase + self.ir_tspill); // x9 = slab bottom (= sp at base)
        let op = if store { "str" } else { "ldr" };
        let mut j = 0u32;
        for r in gp {
            _ = writeln!(self.s, "\t{op} x{r}, [x9, #{}]", 8 * j);
            j += 1;
        }
        for r in fp {
            _ = writeln!(self.s, "\t{op} d{r}, [x9, #{}]", 8 * j);
            j += 1;
        }
    }
    pub(super) fn ld_val(&mut self, v: Val, reg: &str) {
        match v {
            Val::Tmp(t) => self.tmp_load(t, reg),
            Val::Imm(x) => self.imm(reg, x),
            Val::FImm(b) => self.imm(reg, b as i64), // f64 bit pattern in a GPR
        }
    }
    // Tier-1 #1 (compute-into-home) source resolution: the GP register that already HOLDS
    // integer value `v`. A GP-homed temp is read from its home directly (no x0 funnel);
    // anything else (spilled temp / immediate) is materialized into `scratch` and returned.
    // Called on the integer Bin/Un path and the branch-condition path ⟹ v is an integer
    // value (never FP-homed); an FP-homed v would still be handled safely via ld_val's fmov.
    pub(super) fn src_gp(&mut self, v: Val, scratch: u32) -> u32 {
        if let Val::Tmp(t) = v {
            if let Some((false, idx)) = self.talloc.get(t as usize).copied().flatten() {
                return self.gpp(idx);
            }
        }
        self.ld_val(v, &format!("x{scratch}"));
        scratch
    }
    // The GP home register of temp `d` if it is GP-register-resident, else None (spilled).
    pub(super) fn gp_home(&self, d: Tmp) -> Option<u32> {
        match self.talloc.get(d as usize).copied().flatten() {
            Some((false, idx)) => Some(self.gpp(idx)),
            _ => None,
        }
    }
    pub(super) fn ir_label(&self, b: u32) -> String {
        format!(".Lir_{}_{}", self.fname, b)
    }
    pub(super) fn val_is_float(&self, v: Val) -> bool {
        match v {
            Val::FImm(_) => true,
            Val::Imm(_) => false,
            Val::Tmp(t) => self.a.tt.is_float(self.ir_temps[t as usize]),
        }
    }
    // x0 = &global (+ off). Mirrors the GVar arm of addr(): local-exec TLS / GOT (extern
    // or -fPIC non-static) / adrp+:lo12: (local). Flags looked up in ast.globals by name.
    // x{reg} = &global (+ off). Mirrors the GVar arm of addr(): local-exec TLS / GOT (extern
    // or -fPIC non-static) / adrp+:lo12: (local). `reg` is the destination home (§residence:
    // adrp/mrs take ANY register, so a Lea of a global lands straight in the home — no
    // `mov home,x0` funnel). x9 is the fixed large-off scratch (never a home ⟹ ≠ reg).
    // Spilled dst passes reg=0 ⟹ byte-identical to the old x0 path (opt-parity all-spill).
    pub(super) fn lea_global(&mut self, reg: u32, name: &str, off: i64) {
        let (is_tls, is_got) = {
            let gl = self.a.globals.iter().find(|g| g.name.as_str() == name);
            (
                gl.is_some_and(|g| g.is_tls),
                gl.is_some_and(|g| g.is_extern || (self.a.pic && !g.is_static)),
            )
        };
        let r = format!("x{reg}");
        if is_tls {
            _ = writeln!(self.s, "\tmrs {r}, tpidr_el0\n\tadd {r}, {r}, #:tprel_hi12:{name}, lsl #12\n\tadd {r}, {r}, #:tprel_lo12_nc:{name}");
        } else if is_got {
            _ = writeln!(self.s, "\tadrp {r}, :got:{name}\n\tldr {r}, [{r}, :got_lo12:{name}]");
        } else {
            _ = writeln!(self.s, "\tadrp {r}, {name}\n\tadd {r}, {r}, :lo12:{name}");
        }
        if off > 4095 {
            self.imm("x9", off);
            _ = writeln!(self.s, "\tadd {r}, {r}, x9");
        } else if off > 0 {
            _ = writeln!(self.s, "\tadd {r}, {r}, #{off}");
        }
    }

    // x0 = lhs, x1 = rhs → x0 = lhs ⟨op⟩ rhs, canonical per ct. A semantic copy of
    // Node::Bin (shared once the AST path was removed); the Op enum replaces punctuation.
    pub(super) fn ir_bin(&mut self, op: Op, ct: TypeId) {
        // Funnel registers (base-relative — see `fnl`): v = value (x0/x10), a = 2nd operand
        // (x1/x11), q = rem-quotient scratch (x2/x12). d0/d1 are FP scratch (never homes).
        let (v, a, q) = (self.fnl, self.fnl + 1, self.fnl + 2);
        if self.a.tt.is_float(ct) {
            _ = writeln!(self.s, "\tfmov d0, x{v}\n\tfmov d1, x{a}");
            match op {
                Op::Add => _ = writeln!(self.s, "\tfadd d0, d0, d1\n\tfmov x{v}, d0"),
                Op::Sub => _ = writeln!(self.s, "\tfsub d0, d0, d1\n\tfmov x{v}, d0"),
                Op::Mul => _ = writeln!(self.s, "\tfmul d0, d0, d1\n\tfmov x{v}, d0"),
                Op::Div => _ = writeln!(self.s, "\tfdiv d0, d0, d1\n\tfmov x{v}, d0"),
                _ => {
                    let cond = match op {
                        Op::Eq => "eq", Op::Ne => "ne", Op::Lt => "mi",
                        Op::Le => "ls", Op::Gt => "gt", Op::Ge => "ge",
                        _ => unreachable!(),
                    };
                    _ = writeln!(self.s, "\tfcmp d0, d1\n\tcset x{v}, {cond}");
                }
            }
            return;
        }
        let u = self.a.tt.is_unsigned(ct);
        match op {
            Op::Add => _ = writeln!(self.s, "\tadd x{v}, x{v}, x{a}"),
            Op::Sub => _ = writeln!(self.s, "\tsub x{v}, x{v}, x{a}"),
            Op::Mul => _ = writeln!(self.s, "\tmul x{v}, x{v}, x{a}"),
            Op::Div if u => _ = writeln!(self.s, "\tudiv x{v}, x{v}, x{a}"),
            Op::Div => _ = writeln!(self.s, "\tsdiv x{v}, x{v}, x{a}"),
            Op::Rem if u => _ = writeln!(self.s, "\tudiv x{q}, x{v}, x{a}\n\tmsub x{v}, x{q}, x{a}, x{v}"),
            Op::Rem => _ = writeln!(self.s, "\tsdiv x{q}, x{v}, x{a}\n\tmsub x{v}, x{q}, x{a}, x{v}"),
            Op::And => _ = writeln!(self.s, "\tand x{v}, x{v}, x{a}"),
            Op::Or => _ = writeln!(self.s, "\torr x{v}, x{v}, x{a}"),
            Op::Xor => _ = writeln!(self.s, "\teor x{v}, x{v}, x{a}"),
            Op::Shl => _ = writeln!(self.s, "\tlsl x{v}, x{v}, x{a}"),
            Op::Shr if u => _ = writeln!(self.s, "\tlsr x{v}, x{v}, x{a}"),
            Op::Shr => _ = writeln!(self.s, "\tasr x{v}, x{v}, x{a}"),
            _ => {
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                _ = writeln!(self.s, "\tcmp x{v}, x{a}\n\tcset x{v}, {cond}");
                return; // 0/1, no ext needed
            }
        }
        if self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4 {
            self.ext_r(v, ct);
        }
    }

    // Tier-1 #1 — compute-into-home: x{rd} = x{ra} ⟨op⟩ x{rb}, canonical per ct, with NO
    // x0/x1 funnel. A register-parametric transcription of the integer arm of ir_bin: for
    // rd=ra=0,rb=1 it emits BYTE-IDENTICAL asm to `ir_bin` (the x0-funnel), so the -O0 path
    // (all temps spilled ⟹ rd=ra=0,rb=1) is unchanged; only the register-resident path skips
    // the copies. Correctness = ir_bin's (same mnemonic per Op); validated by opt-parity.
    // The rem quotient uses the fnl+2 funnel scratch (x2 NARROW / x12 WIDE) — never a home: the
    // WIDE home set is x0–x7 ∪ x19–x28, NARROW is x19–x28, and x12 (WIDE) / x2 (NARROW) lie
    // outside both. msub reads
    // all sources before writing rd, so rd may alias ra/rb (the allocator only coalesces when
    // the aliased source is dead here). No ext on the compare path (cset yields a clean 0/1).
    pub(super) fn ir_bin_r(&mut self, op: Op, ct: TypeId, rd: u32, ra: u32, rb: u32) {
        let u = self.a.tt.is_unsigned(ct);
        // Value contract (int32): a 4-byte integer lives in the LOW 32 bits of its home; the
        // high bits are DON'T-CARE. So its arithmetic is emitted in w-form — every w-write
        // auto-zeroes bits 32..63, and a w-op reads only the low 32, which are always the
        // correct int value regardless of the operands' high bits. This drops the eager
        // `sxtw` that used to re-canonicalize after EVERY int op (the ~21k-on-sqlite bloat):
        // sign-extension is now emitted only where the high bits are actually observed — an
        // explicit widening Cast (ext_rd reads w{ra}, sxtw) or address-index scaling — never
        // between two int operations. 8-byte ints / pointers stay x-form (already canonical).
        let is4 = self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4;
        let r = if is4 { 'w' } else { 'x' };
        match op {
            Op::Add => _ = writeln!(self.s, "\tadd {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Sub => _ = writeln!(self.s, "\tsub {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Mul => _ = writeln!(self.s, "\tmul {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Div if u => _ = writeln!(self.s, "\tudiv {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Div => _ = writeln!(self.s, "\tsdiv {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Rem if u => {
                let q = self.fnl + 2;
                _ = writeln!(self.s, "\tudiv {r}{q}, {r}{ra}, {r}{rb}\n\tmsub {r}{rd}, {r}{q}, {r}{rb}, {r}{ra}")
            }
            Op::Rem => {
                let q = self.fnl + 2;
                _ = writeln!(self.s, "\tsdiv {r}{q}, {r}{ra}, {r}{rb}\n\tmsub {r}{rd}, {r}{q}, {r}{rb}, {r}{ra}")
            }
            Op::And => _ = writeln!(self.s, "\tand {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Or => _ = writeln!(self.s, "\torr {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Xor => _ = writeln!(self.s, "\teor {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Shl => _ = writeln!(self.s, "\tlsl {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Shr if u => _ = writeln!(self.s, "\tlsr {r}{rd}, {r}{ra}, {r}{rb}"),
            Op::Shr => _ = writeln!(self.s, "\tasr {r}{rd}, {r}{ra}, {r}{rb}"),
            _ => {
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                // int compare: w-form (low-32 signed/unsigned NZCV) is exactly the int semantics.
                let cr = if self.a.tt.is_integer(ct) && self.a.tt.size(ct) <= 4 { 'w' } else { 'x' };
                _ = writeln!(self.s, "\tcmp {cr}{ra}, {cr}{rb}\n\tcset x{rd}, {cond}");
                return; // 0/1, no ext needed
            }
        }
        // No trailing ext_r: the w-op already left a canonical int32 (low-32 value, high-32 zero).
    }

    // Fold a constant right operand into the instruction's immediate field (§4 instruction
    // selection). Returns true and emits `op x{rd}, x{ra}, #imm` (byte-equivalent to the
    // `mov x{scratch},#k; op x{rd},x{ra},x{scratch}` form, since imm() would load exactly #k)
    // when the constant is encodable; false ⟹ caller materializes the operand and falls back
    // to the register form. Handles: relational compares (cmp/cmn imm12 + cset), shifts
    // (imm6), and logical and/orr/eor (ARM bitmask immediate). Add/Sub are handled earlier.
    pub(super) fn try_bin_imm(&mut self, op: Op, ct: TypeId, rd: u32, ra: u32, b: Val) -> bool {
        let Val::Imm(k) = b else { return false };
        let u = self.a.tt.is_unsigned(ct);
        let is4 = self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4;
        match op {
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                // cmp x,#k for 0..4095; cmn x,#m (= cmp against −m) for −4095..0. Both set the
                // NZCV of x−k identically to `cmp x,xK`, so every signed/unsigned cond holds.
                let (mnem, mag) = if (0..4096).contains(&k) {
                    ("cmp", k as u64)
                } else if (-4095..=0).contains(&k) {
                    ("cmn", (-k) as u64)
                } else {
                    return false;
                };
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                // int32: compare the low 32 bits (w-form) — high bits are don't-care (ir_bin_r).
                let cr = if is4 { 'w' } else { 'x' };
                _ = writeln!(self.s, "\t{mnem} {cr}{ra}, #{mag}\n\tcset x{rd}, {cond}");
                true // 0/1 result, no ext
            }
            Op::Shl | Op::Shr => {
                let bits = if is4 { 32 } else { 64 };
                if !(0..bits).contains(&k) {
                    return false;
                }
                let mnem = match op {
                    Op::Shl => "lsl",
                    Op::Shr if u => "lsr",
                    _ => "asr",
                };
                // int32 MUST shift in w-form: a right shift (lsr/asr) in x-form would pull the
                // don't-care high bits into the low-32 result. w-form shifts exactly 32 bits and
                // auto-zeroes high — canonical, no trailing sxtw.
                let r = if is4 { 'w' } else { 'x' };
                _ = writeln!(self.s, "\t{mnem} {r}{rd}, {r}{ra}, #{k}");
                true
            }
            Op::And | Op::Or | Op::Xor => {
                if !is_logical_imm(k as u64) {
                    return false;
                }
                let mnem = match op {
                    Op::And => "and",
                    Op::Or => "orr",
                    _ => "eor",
                };
                // print the 64-bit pattern (k may be negative); the register form would have
                // loaded exactly this pattern via imm(), so the fold is byte-equivalent. x-form
                // is fine for int32: the low 32 bits of the result are the correct int value
                // (op is bitwise), and high bits stay don't-care — no trailing sxtw needed.
                _ = writeln!(self.s, "\t{mnem} x{rd}, x{ra}, #{}", k as u64);
                true
            }
            _ => false,
        }
    }

    // Canonicalize the return value (x0) per self.fret, then place it in the ABI register
    // (a copy of Node::Ret; uses self.fret/self.fsret set by emit_ir).
    pub(super) fn ir_ret_conv(&mut self) {
        match self.a.tt.tys[self.fret as usize] {
            Ty::Double => self.s += "\tfmov d0, x0\n",
            Ty::LDouble => self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n",
            Ty::Float => self.s += "\tfmov d0, x0\n\tfcvt s0, d0\n",
            Ty::Struct(_) => {
                let sz = self.a.tt.size(self.fret);
                if let Some((dbl, n)) = self.a.tt.hfa(self.fret) {
                    self.s += "\tmov x9, x0\n";
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tldr d{j}, [x9, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tldr s{j}, [x9, #{}]", 4 * j);
                        }
                    }
                } else if sz > 16 {
                    let fs = self.fsret;
                    self.lea_local("x9", fs);
                    self.s += "\tldr x1, [x9]\n";
                    self.imm("x2", sz as i64);
                    let n = self.labels(1);
                    _ = writeln!(self.s, "L{n}:");
                    self.s += "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
                    _ = writeln!(self.s, "\tb.ne L{n}");
                } else {
                    self.s += "\tmov x9, x0\n\tldr x0, [x9]\n";
                    if sz > 8 {
                        self.s += "\tldr x1, [x9, #8]\n";
                    }
                }
            }
            _ => {}
        }
    }

    // AAPCS64 scalar-arg marshalling for Inst::Call: GP args → x0..x7, FP args → d0..d7
    // (≤8 each; overflow/composite are routed to ir_call_abi upstream, ir::call_composite).
    //
    // SIMULTANEOUS-COPY semantics (the ABI writes all arg registers "at once"). When every
    // GP source lives OUTSIDE the arg-register file — always true before x0..x7 join the
    // allocatable pool, and for the majority of calls after — this is the straight arg-order
    // sequence, BYTE-IDENTICAL to the historical loop. When register targeting places a GP
    // source INSIDE an arg register that another arg overwrites, a left-to-right sequence
    // would clobber it; we then emit the GP writes as a PARALLEL MOVE: emit any write whose
    // target is not still needed as a source, and break a residual cycle by parking one
    // source in x8 (the indirect-result reg — never a home, never an arg, never touched by
    // ld_val/tmp_load, which use only x9). FP sources are never arg registers, so FP stays
    // in arg order. `⟦·⟧`: the emitted moves realize the same register→register function as
    // the simultaneous copy — a standard parallel-move sequentialization.
    pub(super) fn marshal_call_args(&mut self, args: &[Val]) {
        let (mut gp, mut fp) = (0u32, 0u32);
        let mut kinds: Vec<(bool, u32, Val)> = Vec::with_capacity(args.len());
        for &a in args {
            if self.val_is_float(a) {
                debug_assert!(fp < 8, "Inst::Call fp>8 — call_composite must route to CallX");
                kinds.push((true, fp, a));
                fp += 1;
            } else {
                debug_assert!(gp < 8, "Inst::Call gp>8 — call_composite must route to CallX");
                kinds.push((false, gp, a));
                gp += 1;
            }
        }
        let ngp = gp;
        // Hazard ⟺ some GP arg is sourced from a temp homed in an arg register x{j}, j<ngp.
        let hazard = kinds
            .iter()
            .any(|&(isfp, _, v)| !isfp && matches!(v, Val::Tmp(t) if self.gp_home(t).is_some_and(|p| p < ngp)));
        if !hazard {
            for &(isfp, idx, a) in &kinds {
                if isfp {
                    self.ld_val(a, "x9");
                    _ = writeln!(self.s, "\tfmov d{idx}, x9");
                } else {
                    self.ld_val(a, &format!("x{idx}"));
                }
            }
            return;
        }
        // FP first (sources never in arg regs ⟹ order-independent).
        for &(isfp, idx, a) in &kinds {
            if isfp {
                self.ld_val(a, "x9");
                _ = writeln!(self.s, "\tfmov d{idx}, x9");
            }
        }
        // GP parallel move: (target x{k}, source Val, Some(phys) if GP-reg-homed else None).
        let mut mv: Vec<(u32, Val, Option<u32>)> = kinds
            .iter()
            .filter(|k| !k.0)
            .map(|&(_, k, v)| (k, v, if let Val::Tmp(t) = v { self.gp_home(t) } else { None }))
            .collect();
        let mut done = vec![false; mv.len()];
        let mut left = mv.len();
        while left > 0 {
            let mut progressed = false;
            for i in 0..mv.len() {
                if done[i] {
                    continue;
                }
                let tk = mv[i].0;
                if mv.iter().enumerate().any(|(j, m)| !done[j] && j != i && m.2 == Some(tk)) {
                    continue; // x{tk} is still a live source — writing it now would clobber
                }
                match mv[i].2 {
                    Some(p) => {
                        if p != tk {
                            _ = writeln!(self.s, "\tmov x{tk}, x{p}");
                        }
                    }
                    None => self.ld_val(mv[i].1, &format!("x{tk}")),
                }
                done[i] = true;
                left -= 1;
                progressed = true;
            }
            if !progressed {
                // pure register cycle — park one source in x8, redirect its readers, resume.
                let i = (0..mv.len()).find(|&i| !done[i] && mv[i].2.is_some()).unwrap();
                let p = mv[i].2.unwrap();
                _ = writeln!(self.s, "\tmov x8, x{p}");
                for m in mv.iter_mut() {
                    if m.2 == Some(p) {
                        m.2 = Some(8);
                    }
                }
            }
        }
    }

    pub(super) fn ir_call(&mut self, dst: &Option<Tmp>, callee: &Callee, args: &[Val]) {
        // INVARIANT (ir::call_composite, ir.rs): Inst::Call carries ONLY ≤8 GP + ≤8 FP
        // SCALAR args — any register overflow (gp>8 / fp>8), composite (struct/HFA/>16B),
        // or non-8B float is routed to Inst::CallX/ir_call_abi instead. So neither counter
        // can reach 8 here; the debug_asserts pin that delegated precondition.
        // Snapshot an indirect callee pointer into x17 (a register no home, arg, or marshalling
        // scratch ever touches) BEFORE marshalling: with x6/x7 now allocatable homes, a ≥7-arg
        // call marshals an arg into x6/x7 and would clobber a pointer homed there if read after.
        if let Callee::Ptr(p) = callee {
            self.ld_val(*p, "x17");
        }
        self.marshal_call_args(args);
        match callee {
            Callee::Sym(name) => _ = writeln!(self.s, "\tbl {}", sym(name)),
            Callee::Ptr(_) => self.s += "\tblr x17\n",
        }
        if let Some(d) = dst {
            let rt = self.ir_temps[*d as usize];
            // Canonicalize the return value (matching the AST call): int → ext per width
            // (an extern callee returns w0 with garbage high bits), float → canonical f64.
            let ity = self.a.tt.tys[rt as usize];
            if !self.a.tt.is_float(rt) && !matches!(ity, Ty::Void | Ty::Struct(_)) {
                // integer result sits in x0 (ABI) → extend it straight into d's home, merging
                // the canonicalization and the funnel `mov home,x0` into one extend (§residence).
                let rd = self.gp_home(*d).unwrap_or(0);
                self.ext_rd(rd, 0, rt);
                if self.gp_home(*d).is_none() {
                    self.tmp_store(*d, "x0");
                }
            } else {
                match ity {
                    Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
                    Ty::Double => self.s += "\tfmov x0, d0\n",
                    Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n",
                    _ => {} // Void | Struct: x0 holds the address / nothing
                }
                self.tmp_store(*d, "x0");
            }
        }
    }

    // Full ABI call on IR: a direct PORT of self.call's structure (stack push/pop, AAPCS
    // C.1–C.11) — replacing only `self.expr(arg)` with `ld_val(val, "x0")`, since operands
    // are already materialized as Val (x29-relative temps). A struct Val = an ADDRESS (matching expr).
    // struct return: gather v-regs(HFA)/x0:x1(≤16B)/x8-sret(>16B) into local[sret_off], x0=&local.
    pub(super) fn ir_call_abi(
        &mut self,
        dst: &Option<Tmp>,
        callee: &Callee,
        args: &[(Val, TypeId)],
        ret: TypeId,
        sret_off: u32,
    ) {
        let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
        let (mut gp, mut fp, mut off) = (0u32, 0u32, 0u32);
        let mut plan = Vec::with_capacity(args.len());
        for &(_, t) in args {
            if matches!(self.a.tt.tys[t as usize], Ty::Struct(_)) {
                let sz = self.a.tt.size(t);
                let hfa = self.a.tt.hfa(t);
                if let Some((dbl, n)) = hfa {
                    if fp + n <= 8 {
                        plan.push(ASlot::H(fp, n, dbl));
                        fp += n;
                        continue;
                    }
                    fp = 8; // AAPCS C.3
                }
                let need = if sz > 8 { 2 } else { 1 };
                if hfa.is_none() && gp + need <= 8 {
                    plan.push(ASlot::St(gp, sz > 8));
                    gp += need;
                } else {
                    let o = alup(off, 8);
                    plan.push(ASlot::StS(o, sz));
                    off = o + sz.div_ceil(8) * 8;
                    if hfa.is_none() {
                        gp = 8; // C.11 (an HFA overflow, C.3, does NOT lock NGRN)
                    }
                }
                continue;
            }
            let fl = self.a.tt.is_float(t);
            let szt = self.a.tt.size(t);
            if fl && szt == 16 {
                if fp < 8 {
                    plan.push(ASlot::Q(fp));
                    fp += 1;
                } else {
                    let o = alup(off, 16);
                    plan.push(ASlot::S(o, 16, true));
                    off = o + 16;
                }
            } else if fl && fp < 8 {
                plan.push(ASlot::F(fp, szt == 4));
                fp += 1;
            } else if !fl && gp < 8 {
                plan.push(ASlot::G(gp));
                gp += 1;
            } else {
                let o = alup(off, 8);
                plan.push(ASlot::S(o, szt, fl));
                off = o + 8;
            }
        }
        let pad = (off + 15) & !15;
        // sp is about to leave its base — the `sub sp,#pad` below and the per-arg
        // `str x,[sp,#-16]!` pushes (which displace sp even when pad==0). Loads emitted
        // during marshalling must use the x29-relative form, so disable the sp-fold here
        // and re-enable it once sp is fully restored (after the `add sp,#pad` below).
        self.sp_at_base = false;
        if pad > 0 {
            self.sp_adjust("sub", pad);
        }
        for (&(val, _), &sl) in args.iter().zip(&plan) {
            match sl {
                ASlot::S(o, sz, fl) => {
                    // Stage through x9 (never a home): with x0–x7 now allocatable homes, staging
                    // an arg through x0 would clobber a following arg homed at x0. x8 is the sret
                    // reg (set only after the pop phase), free as scratch here.
                    self.ld_val(val, "x9");
                    if fl && sz == 16 {
                        _ = writeln!(self.s, "\tfmov d0, x9\n\tbl __extenddftf2\n\tstr q0, [sp, #{o}]");
                    } else if fl && sz == 4 {
                        _ = writeln!(self.s, "\tfmov d7, x9\n\tfcvt s7, d7\n\tstr s7, [sp, #{o}]");
                    } else {
                        _ = match sz {
                            1 => writeln!(self.s, "\tstrb w9, [sp, #{o}]"),
                            2 => writeln!(self.s, "\tstrh w9, [sp, #{o}]"),
                            4 => writeln!(self.s, "\tstr w9, [sp, #{o}]"),
                            _ => writeln!(self.s, "\tstr x9, [sp, #{o}]"),
                        };
                    }
                }
                ASlot::StS(o, sz) => {
                    self.ld_val(val, "x9"); // x9 = struct address (home-safe staging)
                    let mut k = 0;
                    while k < sz {
                        _ = writeln!(self.s, "\tldr x8, [x9, #{k}]\n\tstr x8, [sp, #{}]", o + k);
                        k += 8;
                    }
                }
                _ => {}
            }
        }
        if let Callee::Ptr(p) = callee {
            self.ld_val(*p, "x9");
            self.s += "\tstr x9, [sp, #-16]!\n";
        }
        let regargs: Vec<(Val, ASlot)> = args
            .iter()
            .zip(&plan)
            .filter(|(_, sl)| !matches!(sl, ASlot::S(..) | ASlot::StS(..)))
            .map(|(&(v, _), &sl)| (v, sl))
            .collect();
        // COMPOSITE-MARSHAL FAST PATH (#20 struct-by-value / #21 many-args). The push-all/
        // pop-reverse scheme below is a SIMULTANEOUS-COPY sequentialization by stack round-trip:
        // it reads every source (to the stack) before writing any arg-register dest, so a dest
        // write can never clobber an unread source. That is sound but costs a str/ldr pair per
        // reg-arg. When NO reg-arg source lives in a written GP arg register x{p}, p<gp (the
        // common case — struct addresses / values are homed above the arg file), arg-order
        // direct emission realizes the SAME register←source function with no round-trip. This
        // is exactly the no-hazard fast path already proven for scalar calls in marshal_call_args
        // (⟦·⟧: a hazard-free parallel move needs no sequentialization). Fall back to the stack
        // scheme on any hazard, or on any Q reg-arg (long-double `bl __extenddftf2` mid-marshal
        // clobbers the whole caller-saved file, so its source MUST be parked on the stack first).
        let has_q = regargs.iter().any(|(_, sl)| matches!(sl, ASlot::Q(_)));
        let hazard = regargs
            .iter()
            .any(|&(v, _)| matches!(v, Val::Tmp(t) if self.gp_home(t).is_some_and(|p| p < gp)));
        if !has_q && !hazard {
            for &(val, sl) in &regargs {
                match sl {
                    ASlot::G(i) => self.ld_val(val, &format!("x{i}")),
                    ASlot::St(i, two) => {
                        self.ld_val(val, "x9"); // x9 = struct address (scratch, never a home)
                        _ = writeln!(self.s, "\tldr x{i}, [x9]");
                        if two {
                            _ = writeln!(self.s, "\tldr x{}, [x9, #8]", i + 1);
                        }
                    }
                    ASlot::F(i, f32_) => {
                        self.ld_val(val, "x9");
                        _ = writeln!(self.s, "\tfmov d{i}, x9");
                        if f32_ {
                            _ = writeln!(self.s, "\tfcvt s{i}, d{i}");
                        }
                    }
                    ASlot::H(f0, n, dbl) => {
                        self.ld_val(val, "x9");
                        for j in 0..n {
                            if dbl {
                                _ = writeln!(self.s, "\tldr d{}, [x9, #{}]", f0 + j, 8 * j);
                            } else {
                                _ = writeln!(self.s, "\tldr s{}, [x9, #{}]", f0 + j, 4 * j);
                            }
                        }
                    }
                    ASlot::Q(..) | ASlot::S(..) | ASlot::StS(..) => unreachable!(),
                }
            }
        } else {
            for &(val, sl) in &regargs {
                self.ld_val(val, "x9"); // struct: x9 = address (home-safe staging, see ASlot::S)
                if matches!(sl, ASlot::Q(_)) {
                    self.s += "\tfmov d0, x9\n\tbl __extenddftf2\n\tstr q0, [sp, #-16]!\n";
                } else {
                    self.s += "\tstr x9, [sp, #-16]!\n";
                }
            }
            for &(_, sl) in regargs.iter().rev() {
                match sl {
                    ASlot::G(i) => _ = writeln!(self.s, "\tldr x{i}, [sp], #16"),
                    ASlot::F(i, f32_) => {
                        _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tfmov d{i}, x9");
                        if f32_ {
                            _ = writeln!(self.s, "\tfcvt s{i}, d{i}");
                        }
                    }
                    ASlot::St(i, two) => {
                        _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tldr x{i}, [x9]");
                        if two {
                            _ = writeln!(self.s, "\tldr x{}, [x9, #8]", i + 1);
                        }
                    }
                    ASlot::H(f0, n, dbl) => {
                        self.s += "\tldr x9, [sp], #16\n";
                        for j in 0..n {
                            if dbl {
                                _ = writeln!(self.s, "\tldr d{}, [x9, #{}]", f0 + j, 8 * j);
                            } else {
                                _ = writeln!(self.s, "\tldr s{}, [x9, #{}]", f0 + j, 4 * j);
                            }
                        }
                    }
                    ASlot::Q(i) => _ = writeln!(self.s, "\tldr q{i}, [sp], #16"),
                    ASlot::S(..) | ASlot::StS(..) => unreachable!(),
                }
            }
        }
        // struct return >16B: the callee writes directly via x8 (set AFTER popping registers, so it is not clobbered)
        let ret_struct = matches!(self.a.tt.tys[ret as usize], Ty::Struct(_));
        if ret_struct && self.a.tt.size(ret) > 16 && self.a.tt.hfa(ret).is_none() {
            self.lea_local("x8", sret_off);
        }
        match callee {
            Callee::Sym(n) => _ = writeln!(self.s, "\tbl {}", sym(n)),
            Callee::Ptr(_) => self.s += "\tldr x9, [sp], #16\n\tblr x9\n",
        }
        if pad > 0 {
            self.sp_adjust("add", pad);
        }
        self.sp_at_base = true; // sp restored to base; folding valid again
        // canonicalize / gather the result
        match self.a.tt.tys[ret as usize] {
            Ty::Void => {}
            Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
            Ty::Double => self.s += "\tfmov x0, d0\n",
            Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n",
            Ty::Struct(_) => {
                let sz = self.a.tt.size(ret);
                if let Some((dbl, n)) = self.a.tt.hfa(ret) {
                    self.lea_local("x9", sret_off);
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tstr d{j}, [x9, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tstr s{j}, [x9, #{}]", 4 * j);
                        }
                    }
                } else if sz <= 16 {
                    self.lea_local("x9", sret_off);
                    self.s += "\tstr x0, [x9]\n";
                    if sz > 8 {
                        self.s += "\tstr x1, [x9, #8]\n";
                    }
                }
                self.lea_local("x0", sret_off); // value = &local
            }
            _ => self.ext(ret),
        }
        if let Some(d) = dst {
            self.tmp_store(*d, "x0");
        }
    }

    // EXT(gcc) atomics on IR: a PORT of the LL/SC body of self.expr(Node::Sync), replacing
    // arg evaluation with ld_val (operands are already Val). x0=ptr, x1=val, x2=val2; the loop uses x9/x10/x11.
    pub(super) fn ir_sync(&mut self, dst: &Option<Tmp>, op: SyncOp, operands: &[Val], sz: u32, ret: TypeId) {
        // load ALL operands before the loop claims x9 (ld_val uses x9 as an address scratch)
        if let Some(v) = operands.first() {
            self.ld_val(*v, "x0");
        }
        if let Some(v) = operands.get(1) {
            self.ld_val(*v, "x1");
        }
        if let Some(v) = operands.get(2) {
            self.ld_val(*v, "x2");
        }
        let r = if sz == 8 { "x" } else { "w" };
        let unsigned = self.a.tt.is_unsigned(ret);
        let canon = |s: &mut String, res: u32| {
            _ = match (sz, unsigned) {
                (8, _) => writeln!(s, "\tmov x0, x{res}"),
                (_, true) => writeln!(s, "\tmov w0, w{res}"),
                _ => writeln!(s, "\tsxtw x0, w{res}"),
            };
        };
        let n = self.labels(3);
        match op {
            SyncOp::FetchAdd
            | SyncOp::AddFetch
            | SyncOp::FetchSub
            | SyncOp::SubFetch
            | SyncOp::FetchAnd
            | SyncOp::FetchOr
            | SyncOp::FetchXor => {
                let ins = match op {
                    SyncOp::FetchAdd | SyncOp::AddFetch => "add",
                    SyncOp::FetchSub | SyncOp::SubFetch => "sub",
                    SyncOp::FetchAnd => "and",
                    SyncOp::FetchOr => "orr",
                    _ => "eor",
                };
                _ = writeln!(
                    self.s,
                    "L{n}:\n\tldaxr {r}9, [x0]\n\t{ins} {r}10, {r}9, {r}1\n\tstlxr w11, {r}10, [x0]\n\tcbnz w11, L{n}"
                );
                let old = !matches!(op, SyncOp::AddFetch | SyncOp::SubFetch);
                canon(&mut self.s, if old { 9 } else { 10 });
            }
            SyncOp::ValCas | SyncOp::BoolCas => {
                _ = writeln!(
                    self.s,
                    "L{n}:\n\tldaxr {r}9, [x0]\n\tcmp {r}9, {r}1\n\tb.ne L{}\n\tstlxr w11, {r}2, [x0]\n\tcbnz w11, L{n}",
                    n + 1
                );
                if matches!(op, SyncOp::BoolCas) {
                    _ = writeln!(
                        self.s,
                        "\tmov x0, #1\n\tb L{}\nL{}:\n\tclrex\n\tmov x0, #0\nL{}:",
                        n + 2,
                        n + 1,
                        n + 2
                    );
                } else {
                    _ = writeln!(self.s, "\tb L{}\nL{}:\n\tclrex\nL{}:", n + 2, n + 1, n + 2);
                }
                if matches!(op, SyncOp::ValCas) {
                    canon(&mut self.s, 9);
                }
            }
            SyncOp::TestSet => {
                _ = writeln!(
                    self.s,
                    "L{n}:\n\tldaxr {r}9, [x0]\n\tstlxr w11, {r}1, [x0]\n\tcbnz w11, L{n}"
                );
                canon(&mut self.s, 9);
            }
            SyncOp::Release => _ = writeln!(self.s, "\tstlr {r}zr, [x0]"),
            SyncOp::Barrier => self.s += "\tdmb ish\n",
        }
        if let Some(d) = dst {
            self.tmp_store(*d, "x0");
        }
    }

    // EXT(gcc) inline asm on IR: a PORT of the body of self.expr(Node::Asm). Operands are
    // already materialized (op.inp = value/address; op.wb = writeback address) → replacing expr/addr with ld_val.
    pub(super) fn ir_asm(&mut self, tpl: &str, ops: &[crate::ir::AsmIrOp]) {
        // Operands are pushed/popped on the stack (phases below), so sp leaves its base for
        // the duration — disable the sp-fold; the writeback pops restore sp before we return.
        self.sp_at_base = false;
        // register assignment: pin > tied > pool (GP x9.., FP v16.. — caller-saved); mem uses the GP pool
        let (mut gp, mut vp) = (9u32, 16u32);
        let mut regs: Vec<u32> = Vec::with_capacity(ops.len());
        for op in ops {
            let r = if let Some(p) = op.pin {
                p as u32
            } else if let Some(t) = op.tied {
                regs[t as usize]
            } else if op.fp {
                vp += 1;
                vp - 1
            } else {
                gp += 1;
                gp - 1
            };
            regs.push(r);
        }
        let sizes: Vec<u32> = ops.iter().map(|o| self.a.tt.size(o.ty)).collect();
        // phase 1: load inputs/mem-addresses onto the stack (a pure output = inp None → skipped)
        let mut pushed: Vec<usize> = Vec::new();
        for (k, op) in ops.iter().enumerate() {
            if let Some(v) = op.inp {
                self.ld_val(v, "x0");
                self.s += "\tstr x0, [sp, #-16]!\n";
                pushed.push(k);
            }
        }
        // phase 2: pop in reverse into the target registers (FP: double bits → demote to s if size 4)
        for &k in pushed.iter().rev() {
            if ops[k].fp {
                _ = writeln!(self.s, "\tldr d{}, [sp], #16", regs[k]);
                if sizes[k] == 4 {
                    _ = writeln!(self.s, "\tfcvt s{0}, d{0}", regs[k]);
                }
            } else {
                _ = writeln!(self.s, "\tldr x{}, [sp], #16", regs[k]);
            }
        }
        // template substitution: %[xwsd]k → reg, %% → %
        let mut sub = String::new();
        let cs: Vec<char> = tpl.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '%' && i + 1 < cs.len() {
                let (mut j, mut m) = (i + 1, ' ');
                match cs[j] {
                    'x' | 'w' | 's' | 'd' => {
                        m = cs[j];
                        j += 1;
                    }
                    '%' => {
                        sub.push('%');
                        i = j + 1;
                        continue;
                    }
                    _ => {}
                }
                if let Some(d) = cs.get(j).and_then(|c| c.to_digit(10)) {
                    let d = d as usize;
                    let (r, op) = (regs[d], &ops[d]);
                    if op.mem {
                        _ = write!(sub, "[x{r}]");
                    } else if op.fp || m == 's' || m == 'd' {
                        let sgl = m == 's' || (m == ' ' && sizes[d] == 4);
                        _ = write!(sub, "{}{}", if sgl { 's' } else { 'd' }, r);
                    } else {
                        let w = m == 'w' || (m == ' ' && sizes[d] < 8);
                        _ = write!(sub, "{}{}", if w { 'w' } else { 'x' }, r);
                    }
                    i = j + 1;
                    continue;
                }
            }
            sub.push(cs[i]);
            i += 1;
        }
        if !sub.is_empty() {
            _ = writeln!(self.s, "\t{}", sub.replace('\n', "\n\t"));
        }
        // writeback of non-mem outputs (mem writes itself via [xN]): value onto the stack first
        let wb: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].wb.is_some()).collect();
        for &k in &wb {
            if ops[k].fp {
                if sizes[k] == 4 {
                    _ = writeln!(self.s, "\tfcvt d{0}, s{0}", regs[k]);
                }
                _ = writeln!(self.s, "\tstr d{}, [sp, #-16]!", regs[k]);
            } else {
                _ = writeln!(self.s, "\tstr x{}, [sp, #-16]!", regs[k]);
            }
        }
        for &k in wb.iter().rev() {
            self.ld_val(ops[k].wb.unwrap(), "x0"); // destination address
            self.s += "\tmov x1, x0\n\tldr x2, [sp], #16\n";
            self.store(2, ops[k].ty);
        }
        self.sp_at_base = true; // all pushes popped; sp back at base
    }

    pub(super) fn emit_inst(&mut self, i: &Inst) {
        // Funnel-scratch base (0 NARROW / 10 WIDE): the spilled/imm fallback register and the
        // x0-funnel helper carriers relocate here so WIDE homes (x0–x7) survive across them.
        let f = self.fnl;
        match i {
            // φ is an SSA-internal node; out_of_ssa (Stage 3) lowers every φ to copies
            // on the predecessor edges before codegen. Reaching the backend = a bug.
            Inst::Phi(..) => unreachable!("Inst::Phi must be eliminated by out_of_ssa before codegen"),
            Inst::Copy(d, _ty, a) => {
                // Compute-into-home, funnel-free. When d is GP-homed, materialize `a` DIRECTLY
                // into d's home: an Imm becomes `mov dHome,#k` (not `mov x0,#k; mov dHome,x0`),
                // a spilled/global `a` loads straight into dHome, and a GP-homed `a` becomes the
                // single `mov dHome,aHome` (elided when the allocator coalesced d≡a). The spilled/
                // FP-destination path keeps the exact x0 funnel — byte-identical to the -O0
                // all-spill baseline (gp_home(d)=None ⟹ this arm), so ⟦all-spill⟧=⟦opt⟧ (opt-parity).
                if let Some(rd) = self.gp_home(*d) {
                    if !matches!(a, Val::Tmp(t) if self.gp_home(*t) == Some(rd)) {
                        self.ld_val(*a, &format!("x{rd}"));
                    }
                } else {
                    let ra = self.src_gp(*a, f);
                    if ra != f {
                        _ = writeln!(self.s, "\tmov x{f}, x{ra}");
                    }
                    self.tmp_store(*d, &format!("x{f}"));
                }
            }
            // B4 if-conversion: dst = (cond ≠ 0) ? a : b. Home-independent x0/x1/x2 funnel
            // (opt::if_convert produces only non-float scalar Selects). `csel` copies the
            // full 64-bit selected operand; ext_r re-canonicalizes to the result width,
            // mirroring interp's canon(ty, chosen) exactly.
            Inst::Select(d, ty, c, a, b) => {
                // Read cond/a/b from homes (scratch x0/x1/x2 only for spilled/imm), csel into
                // d's home, ext in place — no x0 funnel (§residence). The loads before cmp are
                // flag-neutral (mov/ldr/movz), so the compare's flags survive to the csel.
                let rc = self.src_gp(*c, f);
                // ISA: a 0 select-arm reads the zero register (csel Rn/Rm=31 → xzr), no mov.
                let ra = if matches!(a, Val::Imm(0)) { 31 } else { self.src_gp(*a, f + 1) };
                let rb = if matches!(b, Val::Imm(0)) { 31 } else { self.src_gp(*b, f + 2) };
                let rd = self.gp_home(*d).unwrap_or(f);
                _ = writeln!(self.s, "\tcmp x{rc}, #0\n\tcsel {}, {}, {}, ne", xr(rd), xr(ra), xr(rb));
                // csel copies the full 64-bit selected operand; canon(ty,·) is idempotent on a
                // value ALREADY canonical for ty, so the re-canonicalization is dead when BOTH
                // arms are provably canonical for ty. Provable arms: the xzr arm (Imm(0) → 0,
                // canonical for every width) and a same-typed temp (its home holds canon(ty) by
                // the value contract). A general Imm / cross-typed temp is NOT proven → keep the
                // ext. Sound because a skipped ext never changes bits the kept ext would have
                // (idempotence); the kept case is byte-identical to before. [csel→sxtw = 3,412
                // dead in-place sxtw on sqlite3.c — GCC combine's redundant-extend elimination.]
                // Canonical for `ty` ⟺ same canon-signature: exact TypeId, or two plain integers
                // of equal size & signedness (TyTab is not width-deduped, so int32 temps can carry
                // distinct TypeIds — comparing the (size, unsigned) that DRIVES ext_rd is the true
                // predicate). Bool/Bitfield fall to exact-TypeId only (their canon is not a width).
                // canon(ty,·) as ext_rd realizes it, computed on an i64 — for proving an arm
                // already-canonical. Bitfield/Bool are not plain widths ⟹ excluded (None).
                let canon_i64 = |k: i64| -> Option<i64> {
                    match self.a.tt.tys[*ty as usize] {
                        Ty::Bool | Ty::Bitfield(..) => None,
                        _ => Some(match (self.a.tt.size(*ty), self.a.tt.is_unsigned(*ty)) {
                            (1, false) => k as i8 as i64,
                            (1, true) => k as u8 as i64,
                            (2, false) => k as i16 as i64,
                            (2, true) => k as u16 as i64,
                            (4, false) => k as i32 as i64,
                            (4, true) => k as u32 as i64,
                            _ => k,
                        }),
                    }
                };
                let arm_canon = |v: &Val| match v {
                    Val::Imm(0) => true, // xzr = 0, canonical for every width
                    // a materialized `mov reg,#k` holds exactly k ⟹ canonical iff k == canon(ty,k)
                    Val::Imm(k) => canon_i64(*k) == Some(*k),
                    Val::Tmp(t) => {
                        let ot = self.ir_temps[*t as usize];
                        ot == *ty
                            || (!matches!(self.a.tt.tys[*ty as usize], Ty::Bool | Ty::Bitfield(..))
                                && !matches!(self.a.tt.tys[ot as usize], Ty::Bool | Ty::Bitfield(..))
                                && self.a.tt.size(ot) == self.a.tt.size(*ty)
                                && self.a.tt.is_unsigned(ot) == self.a.tt.is_unsigned(*ty))
                    }
                    _ => false,
                };
                if !(arm_canon(a) && arm_canon(b)) {
                    self.ext_r(rd, *ty);
                }
                if self.gp_home(*d).is_none() {
                    self.tmp_store(*d, &format!("x{f}"));
                }
            }
            Inst::Bin(d, op, ty, a, b) => {
                if self.a.tt.is_float(*ty) {
                    self.ld_val(*a, &format!("x{f}"));
                    self.ld_val(*b, &format!("x{}", f + 1));
                    self.ir_bin(*op, *ty);
                    self.tmp_store(*d, &format!("x{f}"));
                } else if let Some((mnem, mag)) = add_sub_imm12(*op, *b) {
                    // Add/Sub-immediate peephole (Side-II: AAPCS64 imm12 field, unsigned
                    // 0..4096): fold a small constant operand into the instruction instead of
                    // materializing it (`mov x1,#k; add` → `add xD,xA,#k`). Pressure-free —
                    // one fewer scratch live per loop increment; validated by opt-parity.
                    let ra = self.src_gp(*a, f);
                    let rd = self.gp_home(*d).unwrap_or(f);
                    // int32 value contract (see ir_bin_r): 4-byte int in w-form (auto-zeroes
                    // high bits, low-32 correct) → no trailing sxtw. 8-byte/ptr stays x-form.
                    let is4 = self.a.tt.is_integer(*ty) && self.a.tt.size(*ty) == 4;
                    let r = if is4 { 'w' } else { 'x' };
                    _ = writeln!(self.s, "\t{mnem} {r}{rd}, {r}{ra}, #{mag}");
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, &format!("x{f}"));
                    }
                } else {
                    // Tier-1 #1: read operands from their homes, compute into d's home.
                    // Sources first (x0/x1 scratch for spilled/imm), THEN pick rd — if d is
                    // spilled, rd=0 (x0) and the result is stored after; a/b are already
                    // consumed into their own scratch/home so x0-as-rd cannot clobber them.
                    let ra = self.src_gp(*a, f);
                    let rd = self.gp_home(*d).unwrap_or(f);
                    // Immediate-operand instruction-selection fold (§4): a compare/shift/logical
                    // with a constant right operand folds the constant into the instruction's
                    // imm field instead of materializing it into a scratch (`mov x1,#k; op` →
                    // `op …,#k`) — byte-equivalent since imm() would load exactly #k. Kills the
                    // ~14k `mov xR,#N` that feed cmp/shift/bitmask. opt-parity certifies.
                    if !self.try_bin_imm(*op, *ty, rd, ra, *b) {
                        let rb = self.src_gp(*b, f + 1);
                        self.ir_bin_r(*op, *ty, rd, ra, rb);
                    }
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, &format!("x{f}"));
                    }
                }
            }
            Inst::Un(d, u, ty, a) => {
                // Float neg keeps the x0 funnel (fmov round-trip). Integer neg/not is the
                // Tier-1 #1 compute-into-home path: read a from its home, write d's home,
                // ext in place. rd=ra=0 ⟹ byte-identical to the old `neg x0,x0; ext` path.
                if matches!(u, Un::Neg) && self.a.tt.is_float(*ty) {
                    self.ld_val(*a, &format!("x{f}"));
                    _ = writeln!(self.s, "\tfmov d0, x{f}\n\tfneg d0, d0\n\tfmov x{f}, d0");
                    self.tmp_store(*d, &format!("x{f}"));
                } else {
                    let ra = self.src_gp(*a, f);
                    let rd = self.gp_home(*d).unwrap_or(f);
                    // int32 value contract (see ir_bin_r): 4-byte int in w-form (low-32 correct,
                    // high auto-zeroed) needs no sxtw. Sub-word (size<4) still canonicalizes via
                    // ext_r (sxtb/sxth). 8-byte/ptr is x-form, already canonical.
                    let sz = self.a.tt.size(*ty);
                    let r = if self.a.tt.is_integer(*ty) && sz <= 4 { 'w' } else { 'x' };
                    match u {
                        Un::Neg => _ = writeln!(self.s, "\tneg {r}{rd}, {r}{ra}"),
                        Un::BNot => _ = writeln!(self.s, "\tmvn {r}{rd}, {r}{ra}"),
                    }
                    if self.a.tt.is_integer(*ty) && sz < 4 {
                        self.ext_r(rd, *ty); // sub-word: canonicalize to its width
                    }
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, &format!("x{f}"));
                    }
                }
            }
            Inst::Load(d, ty, a) => {
                // Tier-1 #2c: address forwarded to `[base, #off]` (the shared add was skipped).
                if let Val::Tmp(t) = a {
                    if let Some((rbase, off)) = self.imm_fold.get(t).copied() {
                        let rd = self.gp_home(*d).unwrap_or(f);
                        self.load_gp_off(rd, rbase, off, *ty);
                        if self.gp_home(*d).is_none() {
                            self.tmp_store(*d, &format!("x{f}"));
                        }
                        return;
                    }
                }
                if self.simple_gp_load_ty(*ty) {
                    // Tier-1 #2 groundwork: address from its home, load into d's home.
                    let ra = self.src_gp(*a, f);
                    let rd = self.gp_home(*d).unwrap_or(f);
                    self.load_gp(rd, ra, *ty);
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, &format!("x{f}"));
                    }
                } else {
                    self.ld_val(*a, &format!("x{f}"));
                    self.load(*ty);
                    self.tmp_store(*d, &format!("x{f}"));
                }
            }
            Inst::Store(ty, a, v) => {
                // Tier-1 #2c: address forwarded to `[base, #off]` (shared add skipped). imm-0 →
                // wzr/xzr (rv=31); else the value from its home.
                if let Val::Tmp(t) = a {
                    if let Some((rbase, off)) = self.imm_fold.get(t).copied() {
                        let rv = if matches!(v, Val::Imm(0)) { 31 } else { self.src_gp(*v, f) };
                        self.store_gp_off(rv, rbase, off, *ty);
                        return;
                    }
                }
                // Read the value from its home (no `mov x{f},vHome` funnel) — store() is now
                // clobber-free so passing a live home is safe. Address funnels to x{f+1}
                // (store reads [x{f+1}]); loading it there cannot clobber a spilled v in x{f}.
                // ISA: a Store of constant 0 uses the zero register directly (str wzr/xzr),
                // never `mov xN,#0; str xN` (AArch64 XZR/WZR = 0; Rt=31 → zero, not sp).
                if matches!(v, Val::Imm(0)) && self.simple_gp_store_ty(*ty) {
                    self.ld_val(*a, &format!("x{}", f + 1));
                    self.store_z(*ty);
                } else {
                    let rv = self.src_gp(*v, f);
                    self.ld_val(*a, &format!("x{}", f + 1));
                    self.store(rv, *ty);
                }
            }
            Inst::Memcpy(d, s, sz) => {
                self.ld_val(*s, &format!("x{f}")); // src
                self.ld_val(*d, &format!("x{}", f + 1)); // dst
                self.blk_copy(*sz);
            }
            Inst::Lea(d, p) => {
                match p {
                    // Address computed straight into the home — no `mov home,x0` funnel
                    // (§residence): Local = one `sub home,x29,#off` (or sp-fold), Global/Str =
                    // adrp+:lo12: into the home (adrp takes any register). Spilled dst (rd=None)
                    // keeps the x0 path + tmp_store — byte-identical to the -O0 all-spill baseline.
                    Place::Local(off) => {
                        let rd = self.gp_home(*d);
                        let reg = rd.map(|r| format!("x{r}")).unwrap_or_else(|| format!("x{f}"));
                        self.lea_local(&reg, *off);
                        if rd.is_none() {
                            self.tmp_store(*d, &format!("x{f}"));
                        }
                    }
                    Place::Global(name, off) => {
                        let rd = self.gp_home(*d);
                        self.lea_global(rd.unwrap_or(f), name, *off);
                        if rd.is_none() {
                            self.tmp_store(*d, &format!("x{f}"));
                        }
                    }
                    Place::Str(i) => {
                        let rd = self.gp_home(*d);
                        let reg = rd.unwrap_or(f);
                        _ = writeln!(
                            self.s,
                            "\tadrp x{reg}, l_str{0}\n\tadd x{reg}, x{reg}, :lo12:l_str{0}",
                            i
                        );
                        if rd.is_none() {
                            self.tmp_store(*d, &format!("x{f}"));
                        }
                    }
                }
            }
            Inst::Cast(d, from, to, a) => {
                // Integer→integer width cast = a single extend/relocate: read a from its home,
                // land canonically in d's home, no x0 funnel (§value-residence). Float casts and
                // void/struct/array reinterprets keep the x0 funnel (d0-scratch conversions).
                let tt = &self.a.tt;
                let int_cast = !tt.is_float(*from)
                    && !tt.is_float(*to)
                    && !matches!(tt.tys[*to as usize], Ty::Void | Ty::Struct(_) | Ty::Array(..));
                if int_cast {
                    let ra = self.src_gp(*a, f);
                    let rd = self.gp_home(*d).unwrap_or(f);
                    // int32 value contract (see ir_bin_r): a 4-byte int now lives in w-form —
                    // its high 32 bits are DON'T-CARE, NOT the old sign-extended canonical form.
                    // So a WIDENING cast can no longer assume the source is already extended: it
                    // must sign/zero-extend the low `from` bits to fill the register, per the
                    // SOURCE's signedness (ext_rd(from): sxtw for signed, mov w for unsigned).
                    // This is the genuine widening sxtw gcc also emits (~641), moved here from
                    // the old eager after-every-op site. Narrowing/same-width canonicalizes to
                    // the TARGET width as before (w-form / sxtb / sxth). Without this split a
                    // negative int widened to long reads high=0 → a live miscompile (proven: a
                    // non-foldable runtime `(long)(a*b)` returned the low-32 as a positive value).
                    let ext_ty = if tt.size(*to) > tt.size(*from) { *from } else { *to };
                    self.ext_rd(rd, ra, ext_ty);
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, &format!("x{f}"));
                    }
                } else {
                    self.ld_val(*a, &format!("x{f}"));
                    self.cast_op(*from, *to);
                    self.tmp_store(*d, &format!("x{f}"));
                }
            }
            Inst::Call(dst, callee, args, _nfix) => self.ir_call(dst, callee, args),
            Inst::CallX(dst, callee, args, ret, sret) => {
                self.ir_call_abi(dst, callee, args, *ret, *sret)
            }
            Inst::Sync(dst, op, operands, sz, ret) => self.ir_sync(dst, *op, operands, *sz, *ret),
            Inst::Asm(tpl, ops) => self.ir_asm(tpl, ops),
            Inst::FunAddr(d, name) => {
                self.emit_funaddr(name);
                self.tmp_store(*d, &format!("x{f}"));
            }
            Inst::LabelAddr(d, name) => {
                self.emit_labeladdr(name);
                self.tmp_store(*d, &format!("x{f}"));
            }
            Inst::Zero(a, sz) => {
                self.ld_val(*a, &format!("x{f}")); // address → funnel
                self.emit_zero(*sz);
            }
            Inst::VaStart(a) => {
                self.ld_val(*a, &format!("x{f}")); // &ap → funnel
                self.emit_vastart();
            }
            Inst::VaArg(d, a, t, tmp) => {
                self.ld_val(*a, "x0"); // &ap → x0
                self.emit_vaarg(*t, *tmp);
                self.tmp_store(*d, "x0");
            }
            Inst::Overflow(d, op, ta, tb, rt, a, b, rp) => {
                // a→x0, b→x1, rp→x9. tmp_load USES x9 as an address scratch → rp must be
                // loaded into x9 LAST (loading a/b first would clobber x9, but loading rp last
                // means nothing clobbers it afterward). Wrong order = writing the result to the
                // wrong address (GCC PR64006/68381…).
                self.ld_val(*a, "x0");
                self.ld_val(*b, "x1");
                self.ld_val(*rp, "x9");
                self.emit_overflow(*op, *ta, *tb, *rt);
                self.tmp_store(*d, "x0");
            }
            Inst::VaArea(d, off) => {
                _ = writeln!(self.s, "\tadd x{f}, x29, #{off}");
                self.tmp_store(*d, &format!("x{f}"));
            }
            Inst::Param(d, i) => {
                // Deliver a promoted GP parameter's incoming value into temp d's home. The
                // value contract keeps every scalar canonical to its width in a 64-bit
                // register (sxtw for int, etc.), so canonicalize via ext_rd/ext_r. When d has
                // a REGISTER home, canonicalize the arg register STRAIGHT into the home (no
                // funnel) — a PARAM temp is barred from the arg-register colors (ClassBudget.narg
                // excludes them), so its home is always x19–x28, disjoint from the arg registers
                // x0–x7 ⟹ never a read/write alias. A spilled d funnels through x{f} then
                // tmp_store. Emitted at entry-top, before any arg-register clobber.
                let ty = self.ir_temps[*d as usize];
                let reg_home = matches!(self.talloc.get(*d as usize).copied().flatten(), Some((false, _)));
                let dst = match self.talloc.get(*d as usize).copied().flatten() {
                    Some((false, idx)) => self.gpp(idx),
                    _ => f, // spilled (or, defensively, an unexpected fp home) → via the funnel
                };
                match self.param_loc[*i as usize] {
                    ParamLoc::Gp(n) => self.ext_rd(dst, n, ty), // x{dst} = canon(x{n})
                    ParamLoc::Stack(off) => {
                        _ = writeln!(self.s, "\tldr x{dst}, [x29, #{off}]");
                        self.ext_r(dst, ty);
                    }
                    ParamLoc::None => unreachable!("Inst::Param for a non-scalar/absent param"),
                }
                if !reg_home {
                    self.tmp_store(*d, &format!("x{f}")); // spilled: land the funneled value in its slot
                }
            }
            Inst::GotoPtr(a) => {
                self.ld_val(*a, &format!("x{f}"));
                _ = writeln!(self.s, "\tbr x{f}");
            }
            Inst::Alloca(d, size) => {
                self.ld_val(*size, &format!("x{f}")); // byte count
                _ = writeln!(self.s, "\tadd x{f}, x{f}, #15\n\tand x{f}, x{f}, #0xfffffffffffffff0\n\tsub sp, sp, x{f}\n\tmov x{f}, sp");
                self.tmp_store(*d, &format!("x{f}"));
            }
        }
    }

    // `ft` = the block index that PHYSICALLY follows this one (bi+1, or None if last). A
    // successor equal to `ft` falls through with no branch. Block-layout fall-through (§6):
    // the fall-through edge is 0 distance ⟹ any cb(n)z to the ADJACENT next label is always
    // in imm19 range; the far edge takes the ±128MB `b`. Control-flow identity, opt-parity.
    // Compare-branch fusion detector. A block that ends in `Br(Tmp(c), then, els)` whose `c`
    // is produced by the block's LAST instruction as an INTEGER relational compare, used
    // NOWHERE else (use_count==1), is the `cmp;cset;cbnz;b` diamond gcc emits as `cmp;b.cc`.
    // Returns the compare's (op, type, lhs, rhs) so emit_cbr can drop the cset and branch on
    // the flags directly. THEOREM: `cset xD,cc` sets xD=(cc?1:0); `cbnz xD,L`/`cbz xD,L`
    // branch on xD≠0/xD=0 ⟺ cc/¬cc; and xD is dead after (single use) ⟹ the boolean need
    // never be materialized. Requiring the compare to be the LAST inst keeps its NZCV flags
    // live to the branch (nothing emitted between the cmp and the b.cc).
    pub(super) fn cbr_relational(&self, blk: &crate::ir::Block) -> Option<(Op, TypeId, Val, Val)> {
        let Term::Br(Val::Tmp(c), _, _) = &blk.term else { return None };
        if let Some(Inst::Bin(d, op, ct, a, b)) = blk.insts.last()
            && *d == *c
            && self.use_count[*c as usize] == 1
            && !self.a.tt.is_float(*ct)
            && rel_cond(*op, self.a.tt.is_unsigned(*ct)).is_some()
        {
            return Some((*op, *ct, *a, *b));
        }
        None
    }

    // Emit a fused compare-and-branch (the cbr_relational case): the exact `cmp`/`cmn` operand
    // lowering of the Bin relational path, then a conditional `b.cc` (no cset). Fall-through and
    // huge-function (¬near_branch, imm19 ±1MB) handling mirror emit_term's Term::Br arm exactly,
    // substituting b.cc for cbnz and b.¬cc for cbz.
    pub(super) fn emit_cbr(&mut self, op: Op, ct: TypeId, a: Val, b: Val, tb: u32, eb: u32, ft: Option<u32>) {
        let u = self.a.tt.is_unsigned(ct);
        let cc = rel_cond(op, u).unwrap();
        let ra = self.src_gp(a, self.fnl);
        // int32 value contract (see ir_bin_r): a 4-byte int lives in w-form — its high bits are
        // DON'T-CARE (e.g. a `sub w,w,#1` that underflows to -1 leaves high=0, NOT sign). So the
        // compare MUST read the low 32 bits (w-form); an x-form `cmp` here would see that -1 as a
        // large positive and take the wrong branch (proven: torture loop-1's `for(i=2;i>=0;i--)`
        // never terminated). Matches the compare lowering in ir_bin_r / try_bin_imm exactly.
        let cr = if self.a.tt.is_integer(ct) && self.a.tt.size(ct) <= 4 { 'w' } else { 'x' };
        // b: fold small ±imm12 into cmp/cmn (byte-identical to try_bin_imm), else a register.
        if let Val::Imm(k) = b
            && (0..4096).contains(&k)
        {
            _ = writeln!(self.s, "\tcmp {cr}{ra}, #{k}");
        } else if let Val::Imm(k) = b
            && (-4095..=0).contains(&k)
        {
            _ = writeln!(self.s, "\tcmn {cr}{ra}, #{}", -k);
        } else {
            let rb = self.src_gp(b, self.fnl + 1);
            _ = writeln!(self.s, "\tcmp {cr}{ra}, {cr}{rb}");
        }
        let (lt, le) = (self.ir_label(tb), self.ir_label(eb));
        let ncc = inv_cond(cc);
        if ft == Some(eb) {
            // else falls through. NEAR: take THEN on cc, else fall to eb. FAR: `b.cc {lt}` may
            // exceed imm19 — skip an unconditional `b {lt}` on ¬cc to an adjacent label that
            // itself falls into eb (the b reaches ±128MB).
            if self.near_branch {
                _ = writeln!(self.s, "\tb.{cc} {lt}");
            } else {
                let n = self.labels(1);
                _ = writeln!(self.s, "\tb.{ncc} L{n}\n\tb {lt}\nL{n}:");
            }
        } else if ft == Some(tb) {
            // then falls through. NEAR: take ELSE on ¬cc. FAR: skip `b {le}` on cc.
            if self.near_branch {
                _ = writeln!(self.s, "\tb.{ncc} {le}");
            } else {
                let n = self.labels(1);
                _ = writeln!(self.s, "\tb.{cc} L{n}\n\tb {le}\nL{n}:");
            }
        } else if self.near_branch {
            _ = writeln!(self.s, "\tb.{cc} {lt}\n\tb {le}");
        } else {
            let n = self.labels(1);
            _ = writeln!(self.s, "\tb.{ncc} L{n}\n\tb {lt}\nL{n}:\n\tb {le}");
        }
    }

    pub(super) fn emit_term(&mut self, t: &Term, ft: Option<u32>) {
        match t {
            Term::Jmp(b) => {
                if ft != Some(*b) {
                    _ = writeln!(self.s, "\tb {}", self.ir_label(*b));
                }
            }
            Term::Br(c, tb, eb) => {
                // Tier-1 #1 compute-into-home for the branch condition: `cbz` can test ANY
                // register, so test c's HOME directly instead of funnelling it through x0.
                // src_gp returns c's home when GP-resident (no `mov x0,xHome`), else
                // materializes into x0 (rc=0). A C branch condition is integer truthiness
                // (never FP-homed), satisfying src_gp's precondition. Pressure-free.
                let rc = self.src_gp(*c, self.fnl);
                let (lt, le) = (self.ir_label(*tb), self.ir_label(*eb));
                // Mirror emit_cbr's fall-through handling (cbnz ↔ b.cc, cbz ↔ b.¬cc): when a
                // successor is the ADJACENT block, fall into it and spend ONE conditional branch
                // on the other edge — the #17 payoff (a rotated loop's latch then costs a single
                // `cbnz back-edge`, no unconditional `b`). The 2-insn forms survive only where no
                // successor is adjacent, or in a huge function where `then` may exceed imm19.
                if ft == Some(*eb) {
                    // else falls through. NEAR: cbnz THEN on c!=0, fall to eb (1 insn). FAR: cbz
                    // to the adjacent eb (in imm19 range), then `b then` (±128MB).
                    if self.near_branch {
                        _ = writeln!(self.s, "\tcbnz x{rc}, {lt}");
                    } else {
                        _ = writeln!(self.s, "\tcbz x{rc}, {le}\n\tb {lt}");
                    }
                } else if ft == Some(*tb) {
                    // then falls through. NEAR: cbz ELSE on c==0, fall to tb (1 insn). FAR: cbnz
                    // to the adjacent tb (in range), then `b else`.
                    if self.near_branch {
                        _ = writeln!(self.s, "\tcbz x{rc}, {le}");
                    } else {
                        _ = writeln!(self.s, "\tcbnz x{rc}, {lt}\n\tb {le}");
                    }
                } else if self.near_branch {
                    // Small function, neither successor adjacent: branch to `then`, `b` the else.
                    _ = writeln!(self.s, "\tcbnz x{rc}, {lt}\n\tb {le}");
                } else {
                    // Huge function (fuzzer -O0): labels may exceed ±1MB. Reach both with `b`
                    // (±128MB), gated by a conditional skip to an ADJACENT local label in range.
                    let n = self.labels(1);
                    _ = writeln!(self.s, "\tcbz x{rc}, L{n}\n\tb {lt}\nL{n}:\n\tb {le}");
                }
            }
            Term::Ret(v) => {
                match v {
                    Some(v) => {
                        self.ld_val(*v, "x0");
                        self.ir_ret_conv();
                    }
                    None => self.s += "\tmov x0, #0\n",
                }
                self.save_callee(false); // restore callee-saved regs before `mov sp,x29`
                self.s += EPILOGUE;
            }
            // falling off the end of a function is not allowed: seal with a default (like the AST-path blanket)
            Term::Unreachable => {
                self.s += "\tmov x0, #0\n";
                self.save_callee(false);
                self.s += "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";
            }
        }
    }

    // batch#2 recognition: find every `Add(base, widen(index32))` (optionally scaled by a
    // Shl) that feeds ONE simple-GP mem access, and that can fold into a `[base, w-index,
    // extend #s]` operand. Returns (Add-dest → ExtFold, the Cast/Shl temps to skip). Runs
    // AFTER coloring (needs gp_home). SOUNDNESS obligations, all discharged here:
    //   • the Add is single-use and immediately feeds the mem access (adjacency) — deleting
    //     it + reusing the address changes no observation;
    //   • the widening Cast (and the scaling Shl, if any) are single-use — suppressing them
    //     drops now-dead values;
    //   • index32 is LIVE at the Add: it has a use at/after the Add in the same block (or in
    //     the terminator) ⟹ the allocator never reused its home between the Cast and the Add,
    //     so reading its w-register at the operand yields index32's value. If the only later
    //     use is in a successor block we conservatively DECLINE (block-local check) — sound,
    //     just misses some. base and index32 must both be register-homed.
    pub(super) fn compute_ext_folds(&self, irf: &IrFunc) -> (std::collections::HashMap<Tmp, ExtFold>, std::collections::HashSet<Tmp>) {
        use std::collections::{HashMap, HashSet};
        // def map: temp → its defining Inst; a multiply-defined temp is poisoned (removed) so
        // we never fold across a non-unique definition.
        let mut def: HashMap<Tmp, &Inst> = HashMap::new();
        let mut poison: HashSet<Tmp> = HashSet::new();
        for blk in &irf.blocks {
            for ins in &blk.insts {
                if let Some(d) = ir::inst_def(ins) {
                    if def.insert(d, ins).is_some() { poison.insert(d); }
                }
            }
        }
        for p in &poison { def.remove(p); }
        // classify offset temp `o` (single-use): Cast(index32) [shift 0] or Shl(Cast(index32),k)
        // [shift k==log2(asize)] → (index32, signed, shift). asize = the access byte width.
        let classify = |o: Tmp, asize: u32| -> Option<(Tmp, bool, u32, Option<Tmp>)> {
            let widen = |c: &Inst| -> Option<(Tmp, bool)> {
                match c {
                    Inst::Cast(_, from, to, Val::Tmp(src))
                        if self.a.tt.size(*from) == 4 && self.a.tt.size(*to) == 8 =>
                        Some((*src, !self.a.tt.is_unsigned(*from))),
                    _ => None,
                }
            };
            match def.get(&o)? {
                // scale 1: the index IS the byte offset (char array / already scaled). shift 0
                // is always encodable, for any access width.
                c @ Inst::Cast(..) => widen(c).map(|(src, s)| (src, s, 0, None)),
                // scale 2^k: Shl(widen(index), k) with k == log2(asize). The inner Cast (w) is
                // also suppressed; require it single-use.
                Inst::Bin(_, Op::Shl, _, Val::Tmp(w), Val::Imm(k))
                    if *k > 0 && (1u32 << *k) == asize
                        && self.use_count.get(*w as usize).copied().unwrap_or(0) == 1 =>
                {
                    widen(def.get(w)?).map(|(src, s)| (src, s, *k as u32, Some(*w)))
                }
                _ => None,
            }
        };
        let mut folds = HashMap::new();
        let mut skip = HashSet::new();
        for blk in &irf.blocks {
            let body = &blk.insts;
            for (j, ins) in body.iter().enumerate() {
                let Inst::Bin(t, Op::Add, ct, Val::Tmp(x), Val::Tmp(y)) = ins else { continue };
                if !self.is_addr_arith(*ct) || self.use_count.get(*t as usize).copied().unwrap_or(0) != 1 {
                    continue;
                }
                // the Add must be IMMEDIATELY followed by a simple-GP Load/Store of Tmp(t)
                let access = match body.get(j + 1) {
                    Some(Inst::Load(_, lty, Val::Tmp(la))) if la == t && self.simple_gp_load_ty(*lty) => *lty,
                    Some(Inst::Store(sty, Val::Tmp(ta), _)) if ta == t && self.simple_gp_store_ty(*sty) => *sty,
                    _ => continue,
                };
                let asize = self.a.tt.size(access);
                // try each operand as the (single-use) index, the other as base
                for (bb, oo) in [(*x, *y), (*y, *x)] {
                    if self.use_count.get(oo as usize).copied().unwrap_or(0) != 1 { continue; }
                    let Some((src, signed, shift, inner)) = classify(oo, asize) else { continue };
                    let (Some(rbase), Some(rindex)) = (self.gp_home(bb), self.gp_home(src)) else { continue };
                    if !index_live_at(body, &blk.term, src, j) { continue; }
                    folds.insert(*t, ExtFold { base: rbase, index_w: rindex, signed, shift });
                    skip.insert(oo);
                    if let Some(w) = inner { skip.insert(w); }
                    break;
                }
            }
        }
        (folds, skip)
    }

    // Immediate-offset address forwarding (Tier-1 #2c). See the call site for the theorem. Returns
    // (add-dest → (base_home, byte_off), the add-dest temps to skip-emit). Runs AFTER coloring.
    pub(super) fn compute_imm_folds(&self, irf: &IrFunc) -> (std::collections::HashMap<Tmp, (u32, u32)>, std::collections::HashSet<Tmp>) {
        use std::collections::{HashMap, HashSet};
        let mut folds = HashMap::new();
        let mut skip = HashSet::new();
        let mut buf = Vec::new();
        for blk in &irf.blocks {
            let body = &blk.insts;
            for (j, ins) in body.iter().enumerate() {
                let Inst::Bin(t, Op::Add, ct, a, b) = ins else { continue };
                if !self.is_addr_arith(*ct) { continue; }
                let (base, off) = match (a, b) {
                    (Val::Tmp(bs), Val::Imm(n)) | (Val::Imm(n), Val::Tmp(bs)) => {
                        let Ok(o) = u32::try_from(*n) else { continue };
                        (*bs, o)
                    }
                    _ => continue,
                };
                let Some(rbase) = self.gp_home(base) else { continue };
                // Every use of t (in this block) must be a simple-GP mem access of t as address,
                // at a scaled-reachable off; the tally must equal the global use_count (else a
                // use escapes to a successor block or a non-mem consumer — decline).
                let uc = self.use_count.get(*t as usize).copied().unwrap_or(0);
                let mut seen = 0u32;
                let mut ok = true;
                let mut last_pos = j;
                for (k, u) in body[j + 1..].iter().enumerate() {
                    buf.clear();
                    ir::inst_uses(u, &mut buf);
                    if !buf.contains(t) { continue; }
                    let good = match u {
                        Inst::Load(_, lty, Val::Tmp(x)) if x == t =>
                            self.simple_gp_load_ty(*lty) && self.scaled_off(off, self.a.tt.size(*lty)),
                        // exclude a store whose VALUE is t itself (would need t materialized)
                        Inst::Store(sty, Val::Tmp(x), v) if x == t && !matches!(v, Val::Tmp(y) if y == t) =>
                            self.simple_gp_store_ty(*sty) && self.scaled_off(off, self.a.tt.size(*sty)),
                        _ => false,
                    };
                    if !good { ok = false; break; }
                    seen += 1;
                    last_pos = j + 1 + k;
                }
                if ok && seen == uc && seen >= 1 && index_live_at(body, &blk.term, base, last_pos) {
                    folds.insert(*t, (rbase, off));
                    skip.insert(*t);
                }
            }
        }
        (folds, skip)
    }

    pub(super) fn emit_ir_body(&mut self, irf: &IrFunc) {
        // IR temps live BELOW the C frame (and below the 192B variadic-save region if
        // present, which emit_params already subtracted). Parameters already sit in frame
        // slots (emit_params, per ABI) → the body reads them via Var(off)→Load, needing NO param-temp.
        self.ir_tbase = irf.frame + if self.fvariadic { 192 } else { 0 };
        self.ir_temps = irf.temps.clone();
        // Stage 5b: assign each temp a home. regalloc off ⟹ all-spill = the memory model.
        // §3 keystone — pick the GP budget. WIDE (x10–x15 caller homes) is sound iff the body
        // never clobbers x10–x15 as fixed scratch, which by exhaustive enumeration means it
        // contains no Overflow / Sync / VaArg (the prologue struct-copy runs before any home is
        // live; lea_local's large-offset path uses x16), AND no mid-body inline soft-float `bl`.
        // The latter: a long-double Load / Store lowers to `bl __trunctfdf2` / `bl __extenddftf2`
        // (load()/store()), and a `bl` clobbers the entire caller-saved file INCLUDING x10–x15 —
        // the same hazard as a Call, but opt::crossing marks call-crossing ONLY for Call/CallX,
        // so an x10–x15 home live across a long-double Load/Store would be silently clobbered.
        // Force NARROW. (long-double arithmetic is lowered to real Calls → already crossing; the
        // long-double marshalling bls sit inside ir_call/ir_call_abi → crossing-confined; the Ret
        // conversion is in the epilogue with no home live — none of those need the gate.)
        let tt = &self.a.tt;
        let heavy = irf.blocks.iter().flat_map(|b| &b.insts).any(|i| {
            // Asm joins the heavy set: ir_asm's output write-back funnels through x0/x1/x2 +
            // store() with no home-disjoint relocation, and an inline-asm body may clobber the
            // caller-saved file arbitrarily — force NARROW so x0–x7 hold no home across it.
            matches!(i, Inst::Overflow(..) | Inst::Sync(..) | Inst::VaArg(..) | Inst::Asm(..))
                || matches!(i, Inst::Load(_, ty, _) | Inst::Store(ty, _, _)
                    if matches!(tt.tys[*ty as usize], Ty::LDouble))
                // A CallX marshalling a long-double arg/ret emits `bl __extenddftf2`/`__trunctfdf2`
                // MID-marshalling; that bl clobbers the whole caller-saved file (x0–x7 included),
                // destroying a not-yet-staged arg still homed there (torture 20020413-1). Force
                // NARROW so no home lives in x0–x7 across the marshalling bl.
                || matches!(i, Inst::CallX(_, _, args, ret, _)
                    if matches!(tt.tys[*ret as usize], Ty::LDouble)
                        || args.iter().any(|(_, t)| matches!(tt.tys[*t as usize], Ty::LDouble)))
        });
        self.gp_wide = self.regalloc && !heavy;
        // Funnel base moves in lockstep: WIDE homes occupy x0–x7, so the x0-funnel scratch
        // relocates to x10–x15 (heavy ⟹ NARROW ⟹ 0 ⟹ historical x0–x5, byte-identical).
        self.fnl = if self.gp_wide { 10 } else { 0 };
        let gpb = if self.gp_wide { &GP_BUDGET_WIDE } else { &GP_BUDGET };
        self.talloc = if self.regalloc {
            crate::opt::abi_alloc(&self.a.tt, irf, gpb, &FP_BUDGET, self.coalesce)
        } else {
            vec![None; irf.temps.len()]
        };
        // Compact spill slots: only SPILLED temps (talloc[t]==None) consume a stack slot,
        // packed densely. A register-homed temp never calls ir_toff, so its slot is dead.
        // Shrinks the frame from temps.len()*8 to num_spilled*8 — the frame bloat that pushed
        // slot offsets past sp-scaling range into the dynamic `sub x?,x29,x10` form. §5/§3.
        let mut spill_off = vec![0u32; irf.temps.len()];
        let mut ns = 0u32;
        for (i, h) in self.talloc.iter().enumerate() {
            if h.is_none() {
                spill_off[i] = ns * 8;
                ns += 1;
            }
        }
        self.spill_off = spill_off;
        // collect the distinct CALLEE-saved physical registers used (color ≥ ncaller)
        self.csave_gp.clear();
        self.csave_fp.clear();
        for h in self.talloc.clone() {
            match h {
                Some((true, idx)) if idx >= FP_BUDGET.ncaller => {
                    let r = fp_phys(idx);
                    if !self.csave_fp.contains(&r) {
                        self.csave_fp.push(r);
                    }
                }
                Some((false, idx)) if idx >= self.gp_ncaller() => {
                    let r = self.gpp(idx);
                    if !self.csave_gp.contains(&r) {
                        self.csave_gp.push(r);
                    }
                }
                _ => {}
            }
        }
        let tbytes = (ns * 8).next_multiple_of(16);
        let csave = ((self.csave_gp.len() + self.csave_fp.len()) as u32 * 8).next_multiple_of(16);
        self.ir_tspill = tbytes + csave; // reset_sp_base (VLA-dealloc) must also subtract this region
        if self.ir_tspill > 0 {
            self.sp_adjust("sub", self.ir_tspill);
        }
        self.save_callee(true); // spill callee-saved regs into the frame-bottom slab
        self.sp_at_base = true; // sp now at its fixed base (per-function reset; Cg is reused)
        // alloca (like a VLA) displaces sp for the rest of the body but does NOT set has_vla
        // → the sp-fold must be disabled for the whole function (see fdynstack).
        self.fdynstack = irf
            .blocks
            .iter()
            .any(|b| b.insts.iter().any(|i| matches!(i, Inst::Alloca(..))));
        // near_branch: a size ESTIMATE (Article-E heuristic, dated 2026-08 — NOT a proven bound).
        // imm19's hard reach is ±2^18 words = 262,144 insns; the 200k threshold sits under it with
        // ~24% margin. The ×20 per-IR-inst factor covers the fat lowerings the operand-audit
        // flagged (VaArg HFA loop ~15-20, Sync CAS, bitfield RMW); the one lowering it does NOT
        // strictly bound is a Call marshalling many by-value composites. This is SAFE because the
        // failure mode is loud, not silent: a mis-classified far label makes GNU-as reject the
        // cb(n)z with an out-of-range relocation (build fails), never a wrong-target miscompile.
        // No real or fuzzed function (sqlite/musl/csmith/yarpgen) approaches 10k IR insts. The
        // correct-but-deferred fix is a two-pass emit-measure-re-emit; unwarranted until a real
        // program trips the assembler. Correctness is thus independent of this number.
        let est = irf.blocks.iter().map(|b| b.insts.len()).sum::<usize>() * 20 + irf.blocks.len() * 4;
        self.near_branch = est < 200_000;
        // Tier-1 #2: function-wide temp READ counts (the authoritative opt::each_use visitor
        // over a clone — codegen is not hot). A single-use address temp is fold-and-deletable.
        self.use_count = vec![0u32; irf.temps.len()];
        for blk in &irf.blocks {
            for inst in &blk.insts {
                each_use_mut(&mut inst.clone(), |v| {
                    if let Val::Tmp(t) = v {
                        self.use_count[*t as usize] += 1;
                    }
                });
            }
            each_use_term_mut(&mut blk.term.clone(), |v| {
                if let Val::Tmp(t) = v {
                    self.use_count[*t as usize] += 1;
                }
            });
        }
        // batch#2 scaled-indexed-with-extend fold: needs use_count (above) + homes (talloc).
        let (folds, skip) = self.compute_ext_folds(irf);
        self.ext_fold = folds;
        self.ext_skip = skip;
        // Tier-1 #2c — immediate-offset address forwarding. An `Add(base, #off)` (address type)
        // whose EVERY use is a simple-GP Load/Store of the add-dest (at a scaled-reachable off,
        // all in the DEFINING block) is folded into each mem operand as `[base, #off]`, deleting
        // the shared add — the multi-use / non-adjacent generalization of try_fuse_addr's imm arm
        // (which only fires for a single adjacent use). Soundness: base register-homed AND live at
        // the last use of t (index_live_at — so deleting the add, which extends base's live range
        // to the fold sites, never reads a recycled home); `seen == use_count` proves no
        // out-of-block or non-mem use escapes the rewrite. Byte-identical to try_fuse_addr's imm
        // output (load_gp_off/store_gp_off/store_z). Machine translation-validation; opt-parity 0.
        let (imm_folds, imm_skip) = self.compute_imm_folds(irf);
        self.imm_fold = imm_folds;
        self.ext_skip.extend(imm_skip);
        // Two-pass branch-range guard (the correct-but-deferred fix promised above). The
        // `est` heuristic can under-count a giant-frame function (each spilled access lowers
        // to ~4 insns, an expansion `est` does not model), leaving a cb(n)z/b.cc in NEAR form
        // whose imm19 target (±262144 insns reach) is out of range → GNU-as rejects the build.
        // Emit the body once; if the near-form stream is large enough that any branch COULD
        // exceed imm19, discard it and re-emit with far forms. The measure is a newline count,
        // an over-approximation of emitted insns (insns ≤ newlines), so a body left in near
        // form provably has < THRESHOLD insns ⟹ every branch reaches. Re-emit is bounded to
        // the rare pathological function; the common path measures once and keeps near.
        let body_start = self.s.len();
        loop {
        for (bi, blk) in irf.blocks.iter().enumerate() {
            _ = writeln!(self.s, "{}:", self.ir_label(bi as u32));
            // EXT(gcc): a C label at this block → emit `lg_fname.name:` for computed-goto
            // (&&label / goto *). Having a label ⟹ a goto target: C99 6.8.6.1 requires SP=base
            // on every entry (a backward goto from within a VLA scope must deallocate). A goto
            // may NOT jump INTO a VLA scope, so the target is always at depth ≤ the current one
            // → resetting the base is safe.
            let is_label = irf.labels.iter().any(|(_, b)| *b == bi as u32);
            for (name, _) in irf.labels.iter().filter(|(_, b)| *b == bi as u32) {
                _ = writeln!(self.s, "lg_{}.{}:", self.fname, name);
            }
            if is_label && self.fhasvla {
                self.reset_sp_base();
            }
            // Compare-branch fusion: when the terminator branches on the block's last
            // instruction (a single-use relational), withhold that instruction from the body
            // and fold cmp+cset+cbnz into one cmp+b.cc. The truncated slice also stops the
            // addressing-fuse chain from reaching across into the withheld compare.
            let cbr = self.cbr_relational(blk);
            let body_len = blk.insts.len() - cbr.is_some() as usize;
            let body = &blk.insts[..body_len];
            let mut ii = 0;
            while ii < body_len {
                // batch#2: the widening Cast / scaling Shl whose value was absorbed into a
                // `[base, w-index, extend]` operand is now dead — skip emitting it.
                if let Some(d) = ir::inst_def(&body[ii]) {
                    if self.ext_skip.contains(&d) {
                        ii += 1;
                        continue;
                    }
                }
                if let Some(n) = self
                    .try_fuse_addr(body, ii)
                    .or_else(|| self.try_fuse_store_addr(body, ii))
                    .or_else(|| self.try_fuse_madd(body, ii))
                    .or_else(|| self.try_fuse_local(body, ii))
                {
                    ii += n;
                    continue;
                }
                self.emit_inst(&body[ii]);
                ii += 1;
            }
            let ft = (bi + 1 < irf.blocks.len()).then_some(bi as u32 + 1);
            if let Some((op, ct, a, b)) = cbr {
                let Term::Br(_, tb, eb) = &blk.term else { unreachable!() };
                self.emit_cbr(op, ct, a, b, *tb, *eb, ft);
            } else {
                self.emit_term(&blk.term, ft);
            }
        }
        // imm19 reaches ±2^18 = 262144 insns; 240k leaves margin and is ≤ that bound, so
        // any body kept in near form has < 240k newlines ≥ its insn count < 262144 → sound.
        if self.near_branch
            && self.s[body_start..].bytes().filter(|&c| c == b'\n').count() >= 240_000
        {
            self.near_branch = false;
            self.s.truncate(body_start);
            continue; // re-emit this body with far branch forms
        }
        break;
        }
    }
}
