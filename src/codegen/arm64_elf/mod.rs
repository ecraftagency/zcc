// Code generation for AArch64 ELF (Linux). Derived from arm64_darwin.rs — a `diff`
// of the two files is itself the documentation of "how Mach-O and ELF differ" (the
// parallel structure is intentional; do not refactor them apart). Differences: no
// `_` prefix, ELF sections (.text/.data/.bss/.rodata/.tdata/.tbss), :lo12:/:got:
// relocations instead of @PAGE/@GOTPAGE, local-exec TLS (mrs tpidr_el0 + :tprel_*)
// instead of the @TLVPPAGE descriptor, .weak instead of .weak_definition, NO
// .subsections_via_symbols, anonymous variadic arguments passed in x0-x7/v0-v7 like
// named ones (standard AAPCS — dropping the Apple stack-only convention), stack
// scalar arguments in rounded 8-byte slots (dropping natural-alignment packing).
// Semantics are -O0, with a chibicc-style expression evaluator:
// the result is always in x0; a binary op emits the right operand first, pushes it
// to the stack (16 bytes to preserve alignment), emits the left operand, pops the
// right operand into x1, then computes `x0 = x0 op x1`.
//
// Value contract (matching ast.rs): every scalar lives in x0 as a 64-bit "canonical"
// value — integers sign/zero-extended per type, float/double as the f64 BIT PATTERN
// (a float is widened to double on load and narrowed to f32 on store). The node type
// (Ast.types) selects the instruction: signed/unsigned (sdiv/udiv, lt/lo...), float
// (fadd, fcmp). After a 32-bit op the value must be re-canonicalized (sxtw/mov w) to
// preserve integer wrapping semantics.
//
// ABI: integer args in x0-x7, float args in v0-v7 (two separate counters); overflow
// goes to the stack in 8-byte slots. Anonymous variadic arguments go in registers
// like named ones (standard AAPCS). Return: x0 / d0.
// Labels: "L{n}" sequential; "LC{id}" case targets; "lg_{fn}.{name}" goto labels.
use crate::ast::{Ast, GInit, SyncOp, Ty, TypeId, VOID};
use crate::ir::{self, Callee, Inst, IrFunc, Op, Place, Term, Tmp, Un, Val};
use crate::opt::{each_use_mut, each_use_term_mut, AbiHome, ClassBudget};
use std::fmt::Write;

// Stage 5b — AAPCS64 §6.1.1 register files partitioned for `opt::abi_alloc`. The
// SPEC-TABLE side of the pass (the algorithm lives in opt.rs): a color index maps to a
// physical register here. A color ≥ ncaller is callee-saved ⟹ obliges a prologue/
// epilogue save/restore.
//   FP: caller-saved v16–v31 then callee-saved v8–v15 (only d8–d15 preserved across a bl).
// GP allocation budgets — §3 keystone (the caller/callee split). The hazard that once forced
// ncaller=0 is now precisely scoped: the emitter's fixed scratch that CLOBBERS x10–x15 lives in
// exactly three body instructions — Overflow (ext.rs overflow_emit uses x14/x15), Sync, VaArg
// (+ the prologue struct-copy, which runs before any home is live). This is the same clobber
// that hung pr64006 when x14/x15 were pooled blindly (ext.rs was outside the first grep) — but
// the cure is to gate on it, not to abandon the whole caller-saved file. A function FREE of
// those three therefore uses x10–x15 as 6 CALLER-saved homes (WIDE, ncaller=6); a function
// containing one falls back to NARROW (x19–x28 only, ncaller=0). x10–x15 are not argument
// registers, so opening them adds no call-marshalling shuffle hazard, and color_abi's crossing[]
// already confines any call-crossing temp to the callee-saved band.
const GP_BUDGET: ClassBudget = ClassBudget { k: 10, ncaller: 0, narg: 0 }; // NARROW: x19–x28
// WIDE caller file = x0–x7 (colors 0–7, the ARGUMENT/result registers), callee x19–x28 (8–17).
// Opening the arg registers as homes is what enables VALUE-PLACEMENT TARGETING (an arg temp
// homed at x{i} elides its marshal mov; a call result homed at x0 elides its capture mov) — the
// ~27k-mov / ~8% sqlite lever. It reintroduces two call-boundary hazards the prior x10–x15 file
// dodged, both handled: the marshalling shuffle (marshal_call_args' parallel move) and the
// indirect-callee clobber (ir_call snapshots the pointer to x17 before marshalling). crossing[]
// confines any call-crossing temp to the callee band; narg=8 keeps PARAM temps off the arg
// registers (Inst::Param delivery stays a permutation-free arg→home copy). The backend funnel
// scratch that used to live in x0–x5 was RELOCATED to x10–x15 (via the `fnl` base; disjoint from
// this home file AND from the x9 internal address scratch); the heavy paths (Overflow/Sync/VaArg/
// Asm, NARROW-only) keep their x0–x5 funnel — they never run WIDE (fnl=0).
const GP_BUDGET_WIDE: ClassBudget = ClassBudget { k: 18, ncaller: 8, narg: 8 }; // WIDE: x0–x7 | x19–x28
const FP_BUDGET: ClassBudget = ClassBudget { k: 24, ncaller: 16, narg: 0 };
fn fp_phys(idx: u32) -> u32 {
    if idx < FP_BUDGET.ncaller { 16 + idx } else { 8 + (idx - FP_BUDGET.ncaller) }
}
// GP register name in x/w form. Reg 31 is the ZERO register (XZR/WZR) in every operand
// position these helpers emit (load/store Rt, data-processing Rm/Rn-of-flag-forms) — NOT
// sp; callers must never pass 31 where the encoding reads it as sp (add/sub-immediate Rn).
fn xr(n: u32) -> String {
    if n == 31 { "xzr".into() } else { format!("x{n}") }
}
fn wr(n: u32) -> String {
    if n == 31 { "wzr".into() } else { format!("w{n}") }
}
// Add/Sub with a small constant right operand → (mnemonic, magnitude) for the AArch64
// imm12 form. Side-II: the imm12 field is an *unsigned* 0..4096; a negative Add becomes a
// Sub and vice versa. Returns None when the operand is not an in-range immediate.
// Relational Op → AArch64 condition suffix (unsigned picks lo/ls/hi/hs). None ⟹ not a
// comparison. The mapping is identical to ir_bin_r/try_bin_imm's cset cond — the fused
// compare-branch reuses it so `cmp;b.cc` carries the exact condition `cmp;cset;cbnz` did.
fn rel_cond(op: Op, u: bool) -> Option<&'static str> {
    Some(match (op, u) {
        (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
        (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
        (Op::Le, true) => "ls", (Op::Le, false) => "le",
        (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
        (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
        _ => return None,
    })
}

// The negated condition (ARMv8 flips the cond field's low bit). Used when the THEN edge is
// the fall-through, so we branch to ELSE on ¬cond. Total over rel_cond's range.
fn inv_cond(cc: &str) -> &'static str {
    match cc {
        "eq" => "ne", "ne" => "eq",
        "lt" => "ge", "ge" => "lt", "le" => "gt", "gt" => "le",
        "lo" => "hs", "hs" => "lo", "ls" => "hi", "hi" => "ls",
        _ => unreachable!(),
    }
}

fn add_sub_imm12(op: Op, b: Val) -> Option<(&'static str, u64)> {
    let Val::Imm(k) = b else { return None };
    let mnem = match (op, k >= 0) {
        (Op::Add, true) | (Op::Sub, false) => "add",
        (Op::Add, false) | (Op::Sub, true) => "sub",
        _ => return None,
    };
    let mag = k.unsigned_abs();
    (mag < 4096).then_some((mnem, mag))
}

// Side-II: ARMv8 logical-immediate encoding (`and/orr/eor #imm`). A 64-bit value is
// encodable iff it is a rotation of a run of `ones` set bits (0<ones<size) within a
// power-of-two element `size ∈ {2,4,8,16,32,64}`, replicated across the register. Neither
// all-zeros nor all-ones is encodable. We always emit the x-form ⟹ check over 64 bits.
fn is_logical_imm(val: u64) -> bool {
    if val == 0 || val == u64::MAX {
        return false;
    }
    let mut size = 2u32;
    while size <= 64 {
        let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
        let elem = val & mask;
        // val must be `elem` replicated every `size` bits
        let mut replicated = true;
        let mut s = size;
        while s < 64 {
            if (val >> s) & mask != elem {
                replicated = false;
                break;
            }
            s += size;
        }
        if replicated {
            let ones = elem.count_ones();
            if ones == 0 || ones == size {
                return false; // degenerate at this (smallest) element ⟹ not encodable
            }
            // elem must be a rotation of the contiguous low-ones pattern (2^ones − 1)
            let target = (1u64 << ones) - 1;
            let mut rot = elem;
            for _ in 0..size {
                if rot == target {
                    return true;
                }
                rot = ((rot >> 1) | (rot << (size - 1))) & mask; // rotate-right within `size`
            }
            return false;
        }
        size <<= 1;
    }
    false
}

const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

// index32 is LIVE at the Add (body position `add_pos`) iff some later instruction in the
// same block, or the terminator, reads it — proving the allocator kept its home holding
// index32 across the Add, so the extended operand reads the right value.
fn index_live_at(body: &[Inst], term: &Term, src: Tmp, add_pos: usize) -> bool {
    let mut buf = Vec::new();
    for ins in &body[add_pos + 1..] {
        buf.clear();
        ir::inst_uses(ins, &mut buf);
        if buf.contains(&src) {
            return true;
        }
    }
    buf.clear();
    ir::term_uses(term, &mut buf);
    buf.contains(&src)
}

// One scaled-indexed-with-extend memory operand `[x{base}, w{index_w}, <sxtw|uxtw> #shift]`.
#[derive(Clone, Copy)]
struct ExtFold {
    base: u32,    // home register of the address base
    index_w: u32, // home register of the 32-bit index (read as its w-half)
    signed: bool, // sxtw (signed source) vs uxtw (unsigned source)
    shift: u32,   // 0, or log2(access size) — the only two ARM-encodable amounts
}

struct Cg<'a> {
    s: String,
    a: &'a Ast,
    lbl: u32,
    fname: String,
    fret: TypeId,
    fsret: u32, // ≠0: slot holding the x8 pointer (function returning a struct >16B)
    // Current variadic function (for VaStart): named args consumed (gp, fp), named
    // stack bytes, frame — the 192B save area sits DIRECTLY BELOW the frame:
    // [x29-frame-192, x29-frame) = VR 128B then GP 64B; gr_top = x29-frame, vr_top = x29-frame-64
    va: (u32, u32, u32, u32),
    // VLA deallocation (C99 6.8.6.1): base SP = x29 - (frame + variadic?192:0); at a
    // label at VLA-depth 0, SP is restored to base (a goto leaving a VLA scope must deallocate).
    fframe: u32,
    fvariadic: bool,
    fhasvla: bool,
    // IR path: base offset of the temp-slot region (= frame; temp i lives at
    // x29 − (ir_tbase + 8 + i*8)); ir_temps = the type table of the current function's temps.
    ir_tbase: u32,
    ir_temps: Vec<TypeId>,
    // IR mode: size of the temp-slot region (tbytes), located BELOW the C frame. VLA
    // deallocation (reset_sp_base) must additionally subtract this region, otherwise sp
    // returns above the temp region and the next VLA's `sub sp` overwrites temps
    // (GCC PR43220). 0 in AST mode → untouched.
    ir_tspill: u32,
    // Stage 5b — register allocation. `regalloc` gates it (on ⟺ the opt pipeline ran).
    // `talloc[t]` = temp t's home: Some((is_fp, color)) in a physical register, or None
    // = spill (its ir_toff slot — the pre-Stage-5b path). `csave_gp`/`csave_fp` = the
    // distinct CALLEE-saved physical registers used → saved into a frame-bottom slab
    // (the lowest bytes of the temp region, below the slots) and restored before each ret.
    regalloc: bool,
    coalesce: bool, // register-coalescing toggle (biased coloring in abi_alloc)
    talloc: Vec<AbiHome>,
    // Compact spill-slot byte offset per temp: only SPILLED temps (talloc[t]==None) consume a
    // stack slot, packed densely. A register-homed temp never calls ir_toff, so its entry is
    // dead (0). This shrinks the frame from temps.len()*8 to num_spilled*8 — the frame bloat
    // that pushed slot offsets past sp-scaling range into the dynamic `sub x?,x29,x10` form.
    spill_off: Vec<u32>,
    // §3: this function uses the WIDE GP budget (x0–x7 caller-saved homes). Set per function
    // in emit_ir_body from the heavy-instruction scan; drives gpp()/gp_ncaller().
    gp_wide: bool,
    // Funnel-scratch base register for the x0-funnel helpers (ir_bin float / load / store /
    // cast_op / blk_copy / emit_zero / emit_{fun,label}addr / emit_vastart) and the spilled/imm
    // fallback in emit_inst. NARROW ⟹ 0 (funnel = x0–x5, byte-identical to the historical path);
    // WIDE ⟹ 10 (funnel = x10–x15, DISJOINT from the home file x0–x7 ∪ x19–x28 and from the x9
    // internal address scratch). Set in lockstep with gp_wide. This is what lets x0–x7 be homes:
    // the funnel that used to clobber them moves out of the way. Heavy paths force NARROW (fnl=0).
    fnl: u32,
    csave_gp: Vec<u32>,
    csave_fp: Vec<u32>,
    // Tier-1 #2 (addressing-mode fold): function-wide READ count per temp, computed once
    // per function via the authoritative opt::each_use visitor. A temp used exactly once is
    // safe to fold-and-delete (its sole use is the Load whose address it computes).
    use_count: Vec<u32>,
    // Scaled-indexed addressing with extend (batch#2). ARM64 `[Xn, Wm, sxtw{#s}]` folds a
    // 32-bit index's sign/zero-extend + address-add into the memory operand itself. `ext_fold`
    // maps an `Add(base, widen(index32))` dest temp → the (base, w-index, extend, shift) to
    // emit; `ext_skip` names the widening Cast (and any scaling Shl) temps whose value is now
    // absorbed into the operand and must NOT be emitted. Sound iff the Add feeds ONE simple-GP
    // mem access (single-use), the widen/shift are single-use, and index32 is LIVE at the Add
    // (a use at/after it ⟹ the allocator kept its home intact) — see compute_ext_folds.
    ext_fold: std::collections::HashMap<Tmp, ExtFold>,
    ext_skip: std::collections::HashSet<Tmp>,
    // Immediate-offset address forwarding (Tier-1 #2c): add-dest → (base_home, byte_off).
    imm_fold: std::collections::HashMap<Tmp, (u32, u32)>,
    // Backend addressing-fold: local slots normally address as `sub x9,x29,#off; [x9]`.
    // When the frame is fixed (no VLA) AND sp sits at its base (not displaced by call-arg
    // marshalling), the same slot is `[sp,#pos]` with pos = frame_total−off — one folded
    // instruction, no scratch. `sp_at_base` tracks that second condition: ir_call_abi /
    // ir_asm clear it while sp is pushed/subtracted, so a mid-marshalling load falls back
    // to the always-correct x29-relative form. (fhasvla is the first condition.)
    sp_at_base: bool,
    // Set when the function contains an `Inst::Alloca` (__builtin_alloca). alloca does
    // `sub sp,sp,xN` in the body and is NOT reclaimed until the epilogue — so sp leaves its
    // base for the rest of the function, exactly like a VLA, yet the frontend does NOT set
    // has_vla for a bare alloca (no vla_szs entry). The sp-fold must treat it like fhasvla.
    fdynstack: bool,
    // Block-layout (§6): true when the function is small enough that every intra-function
    // label is within cb(n)z's ±1MB imm19 reach ⟹ the 2-insn near branch form is safe.
    near_branch: bool,
    // Step-2 param promotion: `param_loc[i]` = where the ABI delivers parameter i (arg
    // register or caller-stack slot), set by emit_params' ABI walk; read by Inst::Param
    // lowering to deliver the value into the temp's home. `param_ref` = the frame offsets
    // still referenced by an IR `Lea(Local(off))` — a scalar param NOT in this set was
    // promoted to Inst::Param (no slot), so emit_params SKIPS its spill.
    param_loc: Vec<ParamLoc>,
    param_ref: std::collections::HashSet<u32>,
}

// Where the AAPCS64 delivers a promoted scalar parameter (GP integer/pointer only; FP
// params keep the frame-slot path). Gp(n) = argument register x{n}; Stack(off) = the
// caller-frame slot at [x29,#off] (register-file overflow, the 9th+ GP argument).
#[derive(Clone, Copy)]
enum ParamLoc {
    None,
    Gp(u32),
    Stack(u32),
}


fn emit_params(g: &mut Cg, f: &crate::ast::Func) {
    let ast = g.a;
    if f.variadic {
        // AAPCS register-save area: spill ALL 8 q-regs + 8 x-regs (including the
        // named portion — harmless redundancy that avoids branching); must precede
        // parameter spilling (which reads the original registers)
        g.sp_adjust("sub", 192);
        g.imm("x9", (g.fframe + 192) as i64);
        g.s += "\tsub x9, x29, x9\n";
        for i in 0..4u32 {
            _ = writeln!(g.s, "\tstp q{}, q{}, [x9, #{}]", 2 * i, 2 * i + 1, 32 * i);
        }
        for i in 0..4u32 {
            _ = writeln!(
                g.s,
                "\tstp x{}, x{}, [x9, #{}]",
                2 * i,
                2 * i + 1,
                128 + 16 * i
            );
        }
    }
    if f.sret != 0 {
        g.lea_local("x9", f.sret);
        g.s += "\tstr x8, [x9]\n";
    }
    // Spill parameters per ABI: two counters gp/fp; on overflow, re-read from the
    // caller's stack region at [x29 + 16 + boff]. Standard AAPCS: an overflowing
    // scalar takes one rounded 8-byte slot; a composite has alignment 8 and size
    // rounded to 8 (over-alignment aligned(16+) is IGNORED — verified against gcc
    // arm64 asm: named x3,x4 / stack [sp,8], GCC PR92904); a composite overflow
    // locks gp=8 (C.11). This MUST match call() byte-for-byte.
    let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
    let (mut gp, mut fp, mut boff) = (0u32, 0u32, 0u32);
    for (idx, &(off, t)) in f.params.iter().enumerate() {
        // struct by value ≤16B: arrives in 1-2 consecutive GPRs (or on the stack)
        if let Some((dbl, n)) = ast.tt.hfa(t) {
            if fp + n <= 8 {
                g.lea_local("x9", off);
                for j in 0..n {
                    if dbl {
                        _ = writeln!(g.s, "\tstr d{}, [x9, #{}]", fp + j, 8 * j);
                    } else {
                        _ = writeln!(g.s, "\tstr s{}, [x9, #{}]", fp + j, 4 * j);
                    }
                }
                fp += n;
            } else {
                fp = 8; // AAPCS C.3: an HFA overflow locks the remaining v-regs
                let sz = ast.tt.size(t);
                let o = alup(boff, 8);
                boff = o + sz.div_ceil(8) * 8;
                _ = writeln!(g.s, "\tadd x11, x29, #{}", 16 + o);
                g.lea_local("x9", off);
                g.imm("x12", sz as i64);
                let n2 = g.labels(1);
                _ = writeln!(g.s, "L{n2}:");
                g.s += "\tldrb w13, [x11], #1\n\tstrb w13, [x9], #1\n\tsubs x12, x12, #1\n";
                _ = writeln!(g.s, "\tb.ne L{n2}");
            }
            continue;
        }
        if matches!(ast.tt.tys[t as usize], Ty::Struct(_)) {
            let sz = ast.tt.size(t);
            if sz > 16 {
                // >16B: arrives as a POINTER (1 GPR / 1 slot) — copy into a local slot
                if gp < 8 {
                    _ = writeln!(g.s, "\tmov x11, x{gp}");
                    gp += 1;
                } else {
                    let o = alup(boff, 8); // pointer = 8-byte scalar
                    boff = o + 8;
                    _ = writeln!(g.s, "\tldr x11, [x29, #{}]", 16 + o);
                }
                g.lea_local("x9", off);
                g.imm("x12", sz as i64);
                let n = g.labels(1);
                _ = writeln!(g.s, "L{n}:");
                g.s += "\tldrb w13, [x11], #1\n\tstrb w13, [x9], #1\n\tsubs x12, x12, #1\n";
                _ = writeln!(g.s, "\tb.ne L{n}");
                continue;
            }
            let need = if sz > 8 { 2 } else { 1 };
            g.lea_local("x9", off);
            if gp + need <= 8 {
                _ = writeln!(g.s, "\tmov x8, x{gp}");
                g.store_narrow(0, sz.min(8));
                if sz > 8 {
                    _ = writeln!(g.s, "\tmov x8, x{}", gp + 1);
                    g.store_narrow(8, sz - 8);
                }
                gp += need;
            } else {
                let o = alup(boff, 8);
                boff = o + 8 * need;
                gp = 8; // AAPCS C.11: a composite overflow to the stack locks NGRN
                _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + o);
                g.store_narrow(0, sz.min(8));
                if sz > 8 {
                    _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + o + 8);
                    g.store_narrow(8, sz - 8);
                }
            }
            continue;
        }
        let fl = ast.tt.is_float(t);
        if fl && fp < 8 {
            // Addressing-model fix (§5): x29 is the fixed frame pointer, so a register param
            // spills straight to `[x29,#-off]` (one stur) instead of `sub x9,x29,#off; str [x9]`
            // whenever off ≤ 256 (imm9). Identical effective address.
            if off <= 256 {
                match ast.tt.size(t) {
                    4 => _ = writeln!(g.s, "\tstur s{fp}, [x29, #-{off}]"),
                    16 => _ = writeln!(g.s, "\tstur q{fp}, [x29, #-{off}]"),
                    _ => _ = writeln!(g.s, "\tstur d{fp}, [x29, #-{off}]"),
                }
            } else {
                g.lea_local("x9", off);
                match ast.tt.size(t) {
                    4 => _ = writeln!(g.s, "\tstr s{fp}, [x9]"),
                    16 => _ = writeln!(g.s, "\tstr q{fp}, [x9]"), // long double: full binary128
                    _ => _ = writeln!(g.s, "\tstr d{fp}, [x9]"),
                }
            }
            fp += 1;
        } else if !fl && gp < 8 {
            g.param_loc[idx] = ParamLoc::Gp(gp); // arg register (for a promoted Inst::Param)
            if g.param_ref.contains(&off) {
                if off <= 256 {
                    g.store_gp_fp(gp, off, t); // stur w/x{gp}, [x29,#-off] per width
                } else {
                    g.lea_local("x9", off);
                    _ = match ast.tt.size(t) {
                        1 => writeln!(g.s, "\tstrb w{gp}, [x9]"),
                        2 => writeln!(g.s, "\tstrh w{gp}, [x9]"),
                        4 => writeln!(g.s, "\tstr w{gp}, [x9]"),
                        _ => writeln!(g.s, "\tstr x{gp}, [x9]"),
                    };
                }
            } // else: promoted → Inst::Param delivers x{gp} into the home; no spill.
            gp += 1;
        } else {
            // scalar on the caller's stack: rounded 8-byte slot at [x29 + 16 + boff]
            // (standard AAPCS); load at the correct width — the value is in the slot's low bytes
            let sz = ast.tt.size(t);
            if sz == 16 {
                // long double overflow: quad stack arg — slot 16, align 16 (AAPCS B/C)
                let o = alup(boff, 16);
                boff = o + 16;
                g.lea_local("x9", off);
                _ = writeln!(g.s, "\tldr q7, [x29, #{}]\n\tstr q7, [x9]", 16 + o);
                continue;
            }
            let o = alup(boff, 8);
            boff = o + 8;
            let src = 16 + o;
            if !fl {
                g.param_loc[idx] = ParamLoc::Stack(src); // caller slot (for a promoted Inst::Param)
            }
            if g.param_ref.contains(&off) {
                g.lea_local("x9", off);
                if fl && sz == 4 {
                    _ = writeln!(g.s, "\tldr s7, [x29, #{src}]\n\tstr s7, [x9]");
                } else {
                    _ = match sz {
                        1 => writeln!(g.s, "\tldrb w8, [x29, #{src}]\n\tstrb w8, [x9]"),
                        2 => writeln!(g.s, "\tldrh w8, [x29, #{src}]\n\tstrh w8, [x9]"),
                        4 => writeln!(g.s, "\tldr w8, [x29, #{src}]\n\tstr w8, [x9]"),
                        _ => writeln!(g.s, "\tldr x8, [x29, #{src}]\n\tstr x8, [x9]"),
                    };
                }
            } // else: promoted → Inst::Param loads from [x29,#src] into the home; no spill.
        }
    }
    g.va = (gp.min(8), fp.min(8), boff, g.fframe);
}


// Module tail (globals/TLS/strings/weak/aliases/nested-stack) — SHARED by emit()
// (AST) and emit_ir() (IR). Reads ast only, and emits into g.s.
fn emit_module_tail(g: &mut Cg, ast: &Ast) {
    for gl in &ast.globals {
        if gl.is_extern {
            // EXT(gcc): extern weak (musl _DYNAMIC) — the reference must be a weak undef
            if gl.is_weak {
                _ = writeln!(g.s, ".weak {}", gl.name);
            }
            continue;
        }
        let (sz, al) = (ast.tt.size(gl.ty), ast.tt.data_align(gl.ty));
        let globl = if gl.is_static {
            String::new()
        } else if gl.is_weak {
            format!(".weak {}\n", gl.name) // EXT(gcc): .weak subsumes global
        } else {
            format!(".globl {}\n", gl.name)
        };
        if gl.is_tls {
            // ELF TLS: the symbol IS the label in .tdata/.tbss ("awT" = TLS),
            // with no descriptor — accessed via tpidr_el0 + :tprel (see addr())
            match &gl.init {
                GInit::None => {
                    _ = writeln!(
                        g.s,
                        ".section .tbss,\"awT\",@nobits\n{}.p2align {}\n{}:\n\t.space {}",
                        globl,
                        al.trailing_zeros(),
                        gl.name,
                        sz.max(1)
                    );
                }
                init => {
                    _ = writeln!(
                        g.s,
                        ".section .tdata,\"awT\",@progbits\n{}.p2align {}\n{}:",
                        globl,
                        al.trailing_zeros(),
                        gl.name
                    );
                    g.gdata(init, sz);
                }
            }
            continue;
        }
        match &gl.init {
            GInit::None if gl.is_static => {
                // GNU .comm: alignment is in BYTES (Darwin uses log2)
                _ = writeln!(g.s, ".local {0}\n.comm {0},{1},{2}", gl.name, sz.max(1), al);
            }
            GInit::None => {
                // tentative definition → common symbol (multiple TUs each with "int x;" are merged)
                _ = writeln!(g.s, ".comm {},{},{}", gl.name, sz.max(1), al);
            }
            init => {
                _ = writeln!(
                    g.s,
                    ".data\n{}.p2align {}\n{}:",
                    globl,
                    al.trailing_zeros(),
                    gl.name
                );
                g.gdata(init, sz);
            }
        }
    }
    if !ast.strs.is_empty() {
        for (i, bytes) in ast.strs.iter().enumerate() {
            // ELF: plain .rodata for EVERY string — there is no content-to-NUL
            // mergeable dedup to avoid (unlike Darwin __cstring, where a string with an
            // embedded NUL "\0abc" must be split via __const lest the linker merge it wrongly).
            g.s += ".section .rodata\n";
            _ = write!(g.s, "l_str{}:\n\t.asciz \"", i);
            for &b in bytes {
                match b {
                    b'"' | b'\\' => _ = write!(g.s, "\\{}", b as char),
                    0x20..=0x7e => g.s.push(b as char),
                    _ => _ = write!(g.s, "\\{:03o}", b),
                }
            }
            g.s += "\"\n";
        }
    }
    // EXT(gcc): weak prototype — an undef reference is lowered to weak (the link does not require the symbol)
    for w in &ast.weak_decls {
        _ = writeln!(g.s, ".weak {}", w);
    }
    // EXT(gcc): __attribute__((alias)) — musl weak_alias: new symbol = old symbol
    for (new, old, weak) in &ast.aliases {
        let vis = if *weak { ".weak" } else { ".globl" };
        _ = writeln!(g.s, "{} {}\n.set {}, {}", vis, new, new, old);
    }
}

impl Cg<'_> {
    // Emit data for a GInit; sz = size of the region to cover (List inserts .space into gaps)
    fn gdata(&mut self, init: &GInit, sz: u32) {
        _ = match init {
            GInit::Num(v) => match sz {
                1 => writeln!(self.s, "\t.byte {}", *v as u8),
                2 => writeln!(self.s, "\t.short {}", *v as u16),
                4 => writeln!(self.s, "\t.long {}", *v as u32),
                _ => writeln!(self.s, "\t.quad {v}"),
            },
            GInit::Str(i) => writeln!(self.s, "\t.quad l_str{i}"),
            GInit::StrOff(i, k) => writeln!(self.s, "\t.quad l_str{i} + {k}"),
            // \x01 prefix = an internal symbol whose name is already complete (&& label); ELF has no prefix
            GInit::Addr(n, k) => {
                let sym = match n.strip_prefix('\x01') {
                    Some(raw) => raw.to_string(),
                    None => n.to_string(),
                };
                if *k == 0 {
                    writeln!(self.s, "\t.quad {sym}")
                } else {
                    writeln!(self.s, "\t.quad {sym} + {k}")
                }
            }
            GInit::Diff(a, b) => match sz {
                4 => writeln!(self.s, "\t.long {a} - {b}"),
                _ => writeln!(self.s, "\t.quad {a} - {b}"),
            },
            GInit::Bytes(b) => {
                let list: Vec<String> = b.iter().map(|x| x.to_string()).collect();
                writeln!(self.s, "\t.byte {}", list.join(","))
            }
            GInit::List(items) => {
                let mut pos = 0u32;
                for (off, isz, it) in items {
                    if *off > pos {
                        _ = writeln!(self.s, "\t.space {}", off - pos);
                    }
                    self.gdata(it, *isz);
                    pos = off + isz;
                }
                if pos < sz {
                    _ = writeln!(self.s, "\t.space {}", sz - pos);
                }
                Ok(())
            }
            GInit::None => unreachable!(),
        };
    }
    // Write the low `sz` bytes (≤8) of x8 into [x9, #off..] — piece by piece,
    // without touching the adjacent slot (x8 is shifted apart, x9 is preserved)
    fn store_narrow(&mut self, mut off: u32, mut sz: u32) {
        while sz > 0 {
            if sz >= 8 {
                _ = writeln!(self.s, "\tstr x8, [x9, #{off}]");
                off += 8;
                sz -= 8;
            } else if sz >= 4 {
                _ = writeln!(self.s, "\tstr w8, [x9, #{off}]\n\tlsr x8, x8, #32");
                off += 4;
                sz -= 4;
            } else if sz >= 2 {
                _ = writeln!(self.s, "\tstrh w8, [x9, #{off}]\n\tlsr x8, x8, #16");
                off += 2;
                sz -= 2;
            } else {
                _ = writeln!(self.s, "\tstrb w8, [x9, #{off}]");
                off += 1;
                sz -= 1;
            }
        }
    }
    fn labels(&mut self, k: u32) -> u32 {
        let n = self.lbl;
        self.lbl += k;
        n
    }
    fn imm(&mut self, reg: &str, v: i64) {
        let u = v as u64;
        _ = writeln!(self.s, "\tmov {reg}, #{}", u & 0xffff);
        for sh in [16, 32, 48] {
            if (u >> sh) & 0xffff != 0 {
                _ = writeln!(self.s, "\tmovk {reg}, #{}, lsl #{sh}", (u >> sh) & 0xffff);
            }
        }
    }
    // The x29-relative slot at `x29 − off` re-expressed as a POSITIVE sp-relative offset,
    // valid ONLY when (a) the frame is fixed — no VLA, so sp never leaves its base — and
    // (b) sp is currently AT that base (not displaced by call-arg marshalling). Then
    // sp = x29 − frame_total, so sp + (frame_total − off) = x29 − off: the identical byte
    // (machine translation-validation of the fold). Returns Some(pos) only when pos fits an
    // 8-byte-scaled ldr/str immediate (multiple of 8, 0..=32760 — covers every real frame);
    // None ⟹ caller keeps the two-instruction lea_local form. Callers pass only 8-byte
    // (x-form) slots, so the /8 scaling and the %8 test are exact.
    fn sp_slot(&self, off: u32) -> Option<u32> {
        self.sp_slot_sz(off, 8)
    }
    // Size-parametric form (for the local addressing-fold, whose access width is 1/2/4/8).
    // pos must satisfy the ldr/str unsigned-scaled encoding: a multiple of the access size,
    // 0..=size·4095. A misaligned or out-of-range local keeps the two-instruction lea form.
    fn sp_slot_sz(&self, off: u32, sz: u32) -> Option<u32> {
        if self.fhasvla || self.fdynstack || !self.sp_at_base {
            return None;
        }
        let total = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
        let pos = total.checked_sub(off)?;
        (pos % sz == 0 && pos <= sz * 4095).then_some(pos)
    }
    // x29−off re-expressed as a POSITIVE sp+pos for a bare ADDRESS computation (add reg,sp,#pos,
    // one instruction, no x16 materialization). Same base-validity as sp_slot — fixed frame, sp
    // at its base — but the imm is add-immediate's UNSCALED imm12 (0..4095, no size scaling: this
    // is an address, not a scaled memory access). The slots that need it are exactly those with
    // off>4095 (deep in the frame, near sp) — for which pos=total−off is SMALL and fits imm12:
    // sp+pos = (x29−total)+(total−off) = x29−off, the identical byte (translation-validation).
    fn sp_add_slot(&self, off: u32) -> Option<u32> {
        if self.fhasvla || self.fdynstack || !self.sp_at_base {
            return None;
        }
        let total = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
        let pos = total.checked_sub(off)?;
        (pos <= 4095).then_some(pos)
    }
    // reg = x29 - off (off may exceed imm12)
    fn lea_local(&mut self, reg: &str, off: u32) {
        if off <= 4095 {
            _ = writeln!(self.s, "\tsub {reg}, x29, #{off}");
        } else if let Some(pos) = self.sp_add_slot(off) {
            // Deep-frame slot: one `add reg,sp,#pos` instead of `mov x16,#off; sub reg,x29,x16`.
            _ = writeln!(self.s, "\tadd {reg}, sp, #{pos}");
        } else {
            // Large-offset scratch is x16 (IP0), NOT x10: x10–x15 are caller-saved allocation
            // homes in the wide GP budget (§3), so this frame-address path must not clobber
            // one. x16 is ABI scratch (used transiently, no bl between imm and sub → veneer-safe).
            self.imm("x16", off as i64);
            _ = writeln!(self.s, "\tsub {reg}, x29, x16");
        }
    }
    fn sp_adjust(&mut self, op: &str, n: u32) {
        if n <= 4095 {
            _ = writeln!(self.s, "\t{op} sp, sp, #{n}");
        } else {
            self.imm("x16", n as i64); // x16 (IP0), not x10 — see lea_local (§3 wide GP budget)
            _ = writeln!(self.s, "\t{op} sp, sp, x16");
        }
    }
    // C99 6.8.6.1: SP returns to the frame's fixed base = x29 - (frame + variadic reg-save).
    // Used on reaching a depth-0 label in a function with a VLA: every VLA allocated by
    // `sub sp` (a dynamic address) must be reclaimed before the label body continues,
    // otherwise a backward goto in a loop drifts SP ever downward → stack overflow.
    fn reset_sp_base(&mut self) {
        let off = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
        self.lea_local("x9", off);
        _ = writeln!(self.s, "\tmov sp, x9");
    }
    // Re-canonicalize x0 per type (after a 32-bit op / narrowing). Funnel default.
    fn ext(&mut self, t: TypeId) {
        self.ext_r(0, t);
    }
    // Register-parametric re-canonicalization: x{r} = canon(x{r}) per the declared width
    // read from TyTab. r=0 is the x0-funnel default; the compute-into-home path (Tier-1 #1)
    // passes the destination's HOME register so the extension lands in place, no x0 detour.
    // Byte-identical to the old `ext` when r=0 (verified: same mnemonics, same order).
    fn ext_r(&mut self, r: u32, t: TypeId) {
        self.ext_rd(r, r, t);
    }
    // Cast-and-relocate in one step: x{rd} = canon(x{ra}) per width `t`. The ARMv8 extend/
    // extract forms all take a distinct source register (`sxtw x{rd}, w{ra}`), so an integer
    // width-cast whose result is register-homed lands directly in the home with NO x0 funnel
    // (kills both `mov x0,aHome` and `mov dHome,x0`). ext_r is the rd==ra special case.
    fn ext_rd(&mut self, rd: u32, ra: u32, t: TypeId) {
        if matches!(self.a.tt.tys[t as usize], Ty::Bool) {
            _ = writeln!(self.s, "\tcmp x{ra}, #0\n\tcset x{rd}, ne");
            return;
        }
        // Bitfield: truncate to w bits per the base's signedness — the value of (l.m = v)
        // is v AFTER truncation (GCC torture 921016-1). First shift reads ra→rd, then in-place.
        if let Ty::Bitfield(b, _, w) = self.a.tt.tys[t as usize] {
            let sh = 64 - w;
            let op = if self.a.tt.is_unsigned(b) {
                "lsr"
            } else {
                "asr"
            };
            _ = writeln!(self.s, "\tlsl x{rd}, x{ra}, #{sh}\n\t{op} x{rd}, x{rd}, #{sh}");
            return;
        }
        let u = self.a.tt.is_unsigned(t);
        // 8-byte (and other) widths have no extend form: the value is already canonical, so a
        // cast is a plain relocate — one `mov` only when rd≠ra (elided when they coincide).
        match (self.a.tt.size(t), u) {
            (1, false) => _ = writeln!(self.s, "\tsxtb x{rd}, w{ra}"),
            (1, true) => _ = writeln!(self.s, "\tuxtb w{rd}, w{ra}"), // w-write auto-zeroes bits 32..63
            (2, false) => _ = writeln!(self.s, "\tsxth x{rd}, w{ra}"),
            (2, true) => _ = writeln!(self.s, "\tuxth w{rd}, w{ra}"),
            (4, false) => _ = writeln!(self.s, "\tsxtw x{rd}, w{ra}"),
            (4, true) => _ = writeln!(self.s, "\tmov w{rd}, w{ra}"),
            _ => {
                if rd != ra {
                    _ = writeln!(self.s, "\tmov x{rd}, x{ra}");
                }
            }
        }
    }
    fn load(&mut self, t: TypeId) {
        // Funnel value/address in x{v} (base-relative — see `fnl`); s0/d0/q0 are FP scratch.
        let v = self.fnl;
        match self.a.tt.tys[t as usize] {
            Ty::Float => _ = writeln!(self.s, "\tldr s0, [x{v}]\n\tfcvt d0, s0\n\tfmov x{v}, d0"),
            // long double: memory binary128 → narrowed to canonical f64 (libgcc rounds correctly).
            // LDouble Load forces NARROW (heavy scan) ⟹ v=0 here; the `bl` clobbers x10–x15 too.
            Ty::LDouble => _ = writeln!(self.s, "\tldr q0, [x{v}]\n\tbl __trunctfdf2\n\tfmov x{v}, d0"),
            Ty::Bitfield(b, boff, w) => {
                // load the whole containing unit (unsigned), then shift left/right to isolate the field
                _ = match self.a.tt.size(b) {
                    1 => writeln!(self.s, "\tldrb w{v}, [x{v}]"),
                    2 => writeln!(self.s, "\tldrh w{v}, [x{v}]"),
                    4 => writeln!(self.s, "\tldr w{v}, [x{v}]"),
                    _ => writeln!(self.s, "\tldr x{v}, [x{v}]"),
                };
                _ = writeln!(self.s, "\tlsl x{v}, x{v}, #{}", 64 - boff - w);
                let sh = if self.a.tt.is_unsigned(b) {
                    "lsr"
                } else {
                    "asr"
                };
                _ = writeln!(self.s, "\t{sh} x{v}, x{v}, #{}", 64 - w);
            }
            _ => {
                let u = self.a.tt.is_unsigned(t);
                _ = match (self.a.tt.size(t), u) {
                    (1, false) => writeln!(self.s, "\tldrsb x{v}, [x{v}]"),
                    (1, true) => writeln!(self.s, "\tldrb w{v}, [x{v}]"),
                    (2, false) => writeln!(self.s, "\tldrsh x{v}, [x{v}]"),
                    (2, true) => writeln!(self.s, "\tldrh w{v}, [x{v}]"),
                    (4, false) => writeln!(self.s, "\tldrsw x{v}, [x{v}]"),
                    (4, true) => writeln!(self.s, "\tldr w{v}, [x{v}]"),
                    _ => writeln!(self.s, "\tldr x{v}, [x{v}]"),
                };
            }
        }
    }
    // Tier-1 #2 groundwork — simple integer/pointer/Double load INTO a home register:
    // `ldr* xRd, [xRa]`, no x0 funnel. Byte-identical to the generic arm of `load` for
    // rd=ra=0. GATED by `simple_gp_load_ty` (the caller): Float (fcvt-widened), LDouble
    // (q-reg + libcall) and Bitfield (shift-extract) keep the x0 funnel. Double flows here
    // — its 8-byte pattern is a plain GP move (SEMANTICS §1: f64 bits live in a GPR).
    fn load_gp(&mut self, rd: u32, ra: u32, t: TypeId) {
        let u = self.a.tt.is_unsigned(t);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldrsb x{rd}, [x{ra}]"),
            (1, true) => writeln!(self.s, "\tldrb w{rd}, [x{ra}]"),
            (2, false) => writeln!(self.s, "\tldrsh x{rd}, [x{ra}]"),
            (2, true) => writeln!(self.s, "\tldrh w{rd}, [x{ra}]"),
            (4, false) => writeln!(self.s, "\tldrsw x{rd}, [x{ra}]"),
            (4, true) => writeln!(self.s, "\tldr w{rd}, [x{ra}]"),
            _ => writeln!(self.s, "\tldr x{rd}, [x{ra}]"),
        };
    }
    fn simple_gp_load_ty(&self, t: TypeId) -> bool {
        !matches!(
            self.a.tt.tys[t as usize],
            Ty::Float | Ty::LDouble | Ty::Bitfield(..)
        )
    }
    // Local addressing-fold (try_fuse_local): a simple-GP load/store whose frame slot folds
    // straight into `[sp,#pos]`. Load form mirrors `load_gp`, store form mirrors the `_` arm
    // of `store` (plain integer/pointer/Double — the widths that truncate on write). Bool /
    // Float / LDouble / Bitfield stores need pre/post work (cmp-cset, fcvt, libcall, RMW) and
    // are excluded, keeping the eager x1-addressed `store` path.
    fn simple_gp_store_ty(&self, t: TypeId) -> bool {
        !matches!(
            self.a.tt.tys[t as usize],
            Ty::Bool | Ty::Float | Ty::LDouble | Ty::Bitfield(..)
        )
    }
    // An `Add` computes an ADDRESS (so its result feeds a mem operand and its operands are
    // 64-bit) iff its type is a pointer/array, or a plain 8-byte scalar (a `long` used as an
    // address). This gates every base+index addressing fold. Array-typed pointer arithmetic
    // (`is[j]` on a global array — ct = the array type, size ≫ 8) is address arithmetic too;
    // the old `size(ct)==8` gate wrongly rejected it, so array indexing never folded.
    fn is_addr_arith(&self, ct: TypeId) -> bool {
        matches!(self.a.tt.tys[ct as usize], Ty::Ptr(_) | Ty::Array(..)) || self.a.tt.size(ct) == 8
    }
    fn load_gp_sp(&mut self, rd: u32, pos: u32, t: TypeId) {
        let u = self.a.tt.is_unsigned(t);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldrsb x{rd}, [sp, #{pos}]"),
            (1, true) => writeln!(self.s, "\tldrb w{rd}, [sp, #{pos}]"),
            (2, false) => writeln!(self.s, "\tldrsh x{rd}, [sp, #{pos}]"),
            (2, true) => writeln!(self.s, "\tldrh w{rd}, [sp, #{pos}]"),
            (4, false) => writeln!(self.s, "\tldrsw x{rd}, [sp, #{pos}]"),
            (4, true) => writeln!(self.s, "\tldr w{rd}, [sp, #{pos}]"),
            _ => writeln!(self.s, "\tldr x{rd}, [sp, #{pos}]"),
        };
    }
    fn store_gp_sp(&mut self, rv: u32, pos: u32, t: TypeId) {
        let (w, x) = (wr(rv), xr(rv));
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb {w}, [sp, #{pos}]"),
            2 => writeln!(self.s, "\tstrh {w}, [sp, #{pos}]"),
            4 => writeln!(self.s, "\tstr {w}, [sp, #{pos}]"),
            _ => writeln!(self.s, "\tstr {x}, [sp, #{pos}]"),
        };
    }
    // Frame-pointer-relative unscaled forms (ldur/stur, imm9 signed −256..255). x29 is the
    // fixed frame pointer (`mov x29,sp`, never reassigned), so `[x29,#-off]` is the SAME
    // effective address as `sub x9,x29,#off; ldr/str [x9]` in one instruction — used when the
    // sp-relative scaled form is out of range (a large frame) but off ≤ 256. `[sp,#pos]` is
    // preferred when available (positive scaled reaches 32 KB); this catches the tail.
    fn load_gp_fp(&mut self, rd: u32, off: u32, t: TypeId) {
        let u = self.a.tt.is_unsigned(t);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldursb x{rd}, [x29, #-{off}]"),
            (1, true) => writeln!(self.s, "\tldurb w{rd}, [x29, #-{off}]"),
            (2, false) => writeln!(self.s, "\tldursh x{rd}, [x29, #-{off}]"),
            (2, true) => writeln!(self.s, "\tldurh w{rd}, [x29, #-{off}]"),
            (4, false) => writeln!(self.s, "\tldursw x{rd}, [x29, #-{off}]"),
            (4, true) => writeln!(self.s, "\tldur w{rd}, [x29, #-{off}]"),
            _ => writeln!(self.s, "\tldur x{rd}, [x29, #-{off}]"),
        };
    }
    fn store_gp_fp(&mut self, rv: u32, off: u32, t: TypeId) {
        let (w, x) = (wr(rv), xr(rv));
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tsturb {w}, [x29, #-{off}]"),
            2 => writeln!(self.s, "\tsturh {w}, [x29, #-{off}]"),
            4 => writeln!(self.s, "\tstur {w}, [x29, #-{off}]"),
            _ => writeln!(self.s, "\tstur {x}, [x29, #-{off}]"),
        };
    }
    // Scaled base+offset load: x{rd} = *(x{rbase} + off), width per t. The ARMv8 scaled
    // immediate form `[Xn, #imm]` requires imm to be a multiple of the access size, imm/size ≤
    // 4095 — checked by scaled_off. Folds a struct-field `add xB,xB,#off; ldr [xB]` into ONE
    // instruction (§4 maximal munch). rd may alias rbase (base read before rd written).
    fn load_gp_off(&mut self, rd: u32, rbase: u32, off: u32, t: TypeId) {
        let u = self.a.tt.is_unsigned(t);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldrsb x{rd}, [x{rbase}, #{off}]"),
            (1, true) => writeln!(self.s, "\tldrb w{rd}, [x{rbase}, #{off}]"),
            (2, false) => writeln!(self.s, "\tldrsh x{rd}, [x{rbase}, #{off}]"),
            (2, true) => writeln!(self.s, "\tldrh w{rd}, [x{rbase}, #{off}]"),
            (4, false) => writeln!(self.s, "\tldrsw x{rd}, [x{rbase}, #{off}]"),
            (4, true) => writeln!(self.s, "\tldr w{rd}, [x{rbase}, #{off}]"),
            _ => writeln!(self.s, "\tldr x{rd}, [x{rbase}, #{off}]"),
        };
    }
    fn store_gp_off(&mut self, rv: u32, rbase: u32, off: u32, t: TypeId) {
        let (w, x) = (wr(rv), xr(rv)); // rv==31 → wzr/xzr (const-0 store); rv<31 → w{rv}/x{rv}
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb {w}, [x{rbase}, #{off}]"),
            2 => writeln!(self.s, "\tstrh {w}, [x{rbase}, #{off}]"),
            4 => writeln!(self.s, "\tstr {w}, [x{rbase}, #{off}]"),
            _ => writeln!(self.s, "\tstr {x}, [x{rbase}, #{off}]"),
        };
    }
    // ARMv8 scaled-immediate reachability for an access of `size` bytes: off is a non-negative
    // multiple of size and off/size ≤ 4095 (the imm12 field). Side-II.
    fn scaled_off(&self, off: u32, size: u32) -> bool {
        size != 0 && off % size == 0 && off / size <= 4095
    }
    // Tier-1 #2 — register-offset load: x{rd} = *(x{rbase} + x{rindex}), width per t. The
    // ARM64 `[Xn, Xm]` addressing form adds the full 64-bit Xm; it exists for every ldr
    // variant used here (ldr/ldrb/ldrh/ldrsb/ldrsh/ldrsw). rd may alias rbase/rindex (base
    // and index are read before rd is written — a single instruction).
    fn load_idx(&mut self, rd: u32, rbase: u32, rindex: u32, t: TypeId) {
        let u = self.a.tt.is_unsigned(t);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldrsb x{rd}, [x{rbase}, x{rindex}]"),
            (1, true) => writeln!(self.s, "\tldrb w{rd}, [x{rbase}, x{rindex}]"),
            (2, false) => writeln!(self.s, "\tldrsh x{rd}, [x{rbase}, x{rindex}]"),
            (2, true) => writeln!(self.s, "\tldrh w{rd}, [x{rbase}, x{rindex}]"),
            (4, false) => writeln!(self.s, "\tldrsw x{rd}, [x{rbase}, x{rindex}]"),
            (4, true) => writeln!(self.s, "\tldr w{rd}, [x{rbase}, x{rindex}]"),
            _ => writeln!(self.s, "\tldr x{rd}, [x{rbase}, x{rindex}]"),
        };
    }
    // Extended-register forms (batch#2): `[Xn, Wm, sxtw|uxtw {#s}]` — the index is a 32-bit
    // Wm, extended (sign/zero) and optionally shifted by log2(access), all inside the operand.
    // `#0` is elided (bare `sxtw`); s is 0 or log2(size), the only ARM-encodable amounts.
    fn ext_suffix(f: &ExtFold) -> String {
        let e = if f.signed { "sxtw" } else { "uxtw" };
        if f.shift == 0 { format!(", {e}") } else { format!(", {e} #{}", f.shift) }
    }
    fn load_idx_ext(&mut self, rd: u32, f: &ExtFold, t: TypeId) {
        let (sfx, u) = (Self::ext_suffix(f), self.a.tt.is_unsigned(t));
        let (b, w) = (f.base, f.index_w);
        _ = match (self.a.tt.size(t), u) {
            (1, false) => writeln!(self.s, "\tldrsb x{rd}, [x{b}, w{w}{sfx}]"),
            (1, true) => writeln!(self.s, "\tldrb w{rd}, [x{b}, w{w}{sfx}]"),
            (2, false) => writeln!(self.s, "\tldrsh x{rd}, [x{b}, w{w}{sfx}]"),
            (2, true) => writeln!(self.s, "\tldrh w{rd}, [x{b}, w{w}{sfx}]"),
            (4, false) => writeln!(self.s, "\tldrsw x{rd}, [x{b}, w{w}{sfx}]"),
            (4, true) => writeln!(self.s, "\tldr w{rd}, [x{b}, w{w}{sfx}]"),
            _ => writeln!(self.s, "\tldr x{rd}, [x{b}, w{w}{sfx}]"),
        };
    }
    fn store_idx_ext(&mut self, rv: u32, f: &ExtFold, t: TypeId) {
        let (sfx, b, w) = (Self::ext_suffix(f), f.base, f.index_w);
        let (wv, xv) = (wr(rv), xr(rv)); // rv==31 → wzr/xzr (const-0 store)
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb {wv}, [x{b}, w{w}{sfx}]"),
            2 => writeln!(self.s, "\tstrh {wv}, [x{b}, w{w}{sfx}]"),
            4 => writeln!(self.s, "\tstr {wv}, [x{b}, w{w}{sfx}]"),
            _ => writeln!(self.s, "\tstr {xv}, [x{b}, w{w}{sfx}]"),
        };
    }
    // Register-offset store (the plain `[Xn, Xm]` form — the store counterpart of load_idx,
    // which was missing). Both 64-bit; value read first (store never clobbers its inputs).
    fn store_idx(&mut self, rv: u32, rbase: u32, rindex: u32, t: TypeId) {
        let (wv, xv) = (wr(rv), xr(rv)); // rv==31 → wzr/xzr (const-0 store)
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb {wv}, [x{rbase}, x{rindex}]"),
            2 => writeln!(self.s, "\tstrh {wv}, [x{rbase}, x{rindex}]"),
            4 => writeln!(self.s, "\tstr {wv}, [x{rbase}, x{rindex}]"),
            _ => writeln!(self.s, "\tstr {xv}, [x{rbase}, x{rindex}]"),
        };
    }
    // Tier-1 #2 — addressing-mode fold (BURS / maximal munch). Recognize the tree
    // `Load(Add(base, index))` when the Add's result feeds ONLY that Load and both operands
    // are register-resident, and emit ONE register-offset load — deleting the separate `add`.
    //   insts[i]   = Bin(t, Add, ct, Tmp(base), Tmp(index)), ct an 8-byte (address) type
    //   insts[i+1] = Load(d, lty, Tmp(t)), lty a simple-GP load, use_count[t] == 1
    // Semantics: `[base+index]` is the same effective address the add computed, and the add
    // is dead (single-use) ⟹ deleting it changes no observation. `⟦·⟧` preserved; validated
    // by opt-parity. The 8-byte-ct gate rules out a narrowing add (an address is never
    // narrowed); `reg_uses` counts BOTH bracket registers as reads, so the peephole that
    // runs later cannot mistake the index for dead. Returns Some(2) on a fold, else None.
    fn try_fuse_addr(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
        let Inst::Bin(t, Op::Add, ct, a, b) = &insts[i] else {
            return None;
        };
        if !self.is_addr_arith(*ct) {
            return None;
        }
        // the address temp must feed exactly one Load, which must be simple-GP-widthed
        let Some(Inst::Load(d, lty, Val::Tmp(la))) = insts.get(i + 1) else {
            return None;
        };
        if *la != *t || !self.simple_gp_load_ty(*lty) {
            return None;
        }
        if self.use_count.get(*t as usize).copied().unwrap_or(0) != 1 {
            return None;
        }
        // base + immediate byte-offset (struct-field access): fold to `ldr [base, #off]` when
        // the offset is scaled-reachable. Add is commutative, so the Imm may be either operand.
        let imm_form = match (a, b) {
            (Val::Tmp(base), Val::Imm(n)) | (Val::Imm(n), Val::Tmp(base)) => {
                let (Some(rbase), Ok(off)) = (self.gp_home(*base), u32::try_from(*n)) else {
                    return None;
                };
                self.scaled_off(off, self.a.tt.size(*lty)).then_some((rbase, off))
            }
            _ => None,
        };
        let rd = self.gp_home(*d).unwrap_or(self.fnl);
        if let Some((rbase, off)) = imm_form {
            self.load_gp_off(rd, rbase, off, *lty);
        } else if let Some(f) = self.ext_fold.get(t).copied() {
            // batch#2: `ldr rd, [base, w-index, extend #s]` — the widening Cast that produced
            // the index is skipped in the emit loop (ext_skip).
            self.load_idx_ext(rd, &f, *lty);
        } else {
            // base + index register form: `ldr [base, index]`
            let (Val::Tmp(ta), Val::Tmp(tb)) = (a, b) else {
                return None;
            };
            let (Some(rbase), Some(rindex)) = (self.gp_home(*ta), self.gp_home(*tb)) else {
                return None;
            };
            self.load_idx(rd, rbase, rindex, *lty);
        }
        if self.gp_home(*d).is_none() {
            self.tmp_store(*d, &format!("x{}", self.fnl));
        }
        Some(2)
    }
    // Tier-1 #3 — multiply-add fusion. Recognize `Add(Mul(x,y), c)` (commutative: the mul
    // may be either add operand) where the Mul feeds ONLY that Add (`use_count==1`), both
    // integer and the SAME width, and emit one `madd xD, xX, xY, xC` = c + x·y — deleting
    // the separate `mul`.
    //   insts[i]   = Bin(m, Mul, ctm, x, y)
    //   insts[i+1] = Bin(d, Add, ctd, {m,c} | {c,m}),  size(ctm)==size(ctd), use_count[m]==1
    // `⟦·⟧` preserved by a ℤ/2ⁿ argument: the original truncates the product to n bits
    // (`mul;ext`) before adding, madd keeps the full 64-bit product — but the FINAL `ext_r`
    // to width n makes them equal, since `(c + trunc_n(x·y)) ≡ (c + x·y) (mod 2ⁿ)` (addition
    // commutes with mod; the low n bits, all `ext_r` observes, are identical). Signedness is
    // irrelevant to `mul`'s low bits. Scratch x0/x1/x2 for spilled/imm operands (never homes).
    // Store counterpart of the base+immediate fold: `add xB,xB,#off; str rv,[xB]` (a struct
    // field WRITE) → one `str rv,[xB,#off]` (§4). Same scaled-reachability + single-use guard.
    // Simple-GP store widths only (Bool/Float/Bitfield/LDouble keep their special [x1] path).
    fn try_fuse_store_addr(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
        let Inst::Bin(t, Op::Add, ct, a, b) = &insts[i] else {
            return None;
        };
        if !self.is_addr_arith(*ct) {
            return None;
        }
        let Some(Inst::Store(sty, Val::Tmp(ta), v)) = insts.get(i + 1) else {
            return None;
        };
        if *ta != *t || !self.simple_gp_store_ty(*sty) {
            return None;
        }
        if self.use_count.get(*t as usize).copied().unwrap_or(0) != 1 {
            return None;
        }
        // base + immediate byte-offset: `str rv, [base, #off]` (struct-field write).
        if let (Val::Tmp(base), Val::Imm(n)) | (Val::Imm(n), Val::Tmp(base)) = (a, b) {
            let (Some(rbase), Ok(off)) = (self.gp_home(*base), u32::try_from(*n)) else {
                return None;
            };
            if !self.scaled_off(off, self.a.tt.size(*sty)) {
                return None;
            }
            let rv = if matches!(v, Val::Imm(0)) { 31 } else { self.src_gp(*v, self.fnl) };
            self.store_gp_off(rv, rbase, off, *sty);
            return Some(2);
        }
        // batch#2: `str rv, [base, w-index, extend #s]` (the widening Cast is skipped).
        if let Some(f) = self.ext_fold.get(t).copied() {
            let rv = if matches!(v, Val::Imm(0)) { 31 } else { self.src_gp(*v, self.fnl) };
            self.store_idx_ext(rv, &f, *sty);
            return Some(2);
        }
        // base + index register form: `str rv, [base, index]` (the store counterpart of
        // load_idx, previously missing — the sieve `is[j]=0` inner store).
        let (Val::Tmp(ta), Val::Tmp(tb)) = (a, b) else {
            return None;
        };
        let (Some(rbase), Some(rindex)) = (self.gp_home(*ta), self.gp_home(*tb)) else {
            return None;
        };
        let rv = if matches!(v, Val::Imm(0)) { 31 } else { self.src_gp(*v, self.fnl) };
        self.store_idx(rv, rbase, rindex, *sty);
        Some(2)
    }
    fn try_fuse_madd(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
        let Inst::Bin(m, Op::Mul, ctm, mx, my) = &insts[i] else {
            return None;
        };
        if !self.a.tt.is_integer(*ctm) {
            return None;
        }
        let Some(Inst::Bin(d, Op::Add, ctd, aa, bb)) = insts.get(i + 1) else {
            return None;
        };
        if !self.a.tt.is_integer(*ctd) || self.a.tt.size(*ctm) != self.a.tt.size(*ctd) {
            return None;
        }
        let addend = match (aa, bb) {
            (Val::Tmp(t), _) if t == m => *bb,
            (_, Val::Tmp(t)) if t == m => *aa,
            _ => return None,
        };
        if self.use_count.get(*m as usize).copied().unwrap_or(0) != 1 {
            return None;
        }
        let fnl = self.fnl;
        let rx = self.src_gp(*mx, fnl);
        let ry = self.src_gp(*my, fnl + 1);
        let ra = self.src_gp(addend, fnl + 2);
        let rd = self.gp_home(*d).unwrap_or(fnl);
        _ = writeln!(self.s, "\tmadd x{rd}, x{rx}, x{ry}, x{ra}");
        self.ext_r(rd, *ctd);
        if self.gp_home(*d).is_none() {
            self.tmp_store(*d, &format!("x{fnl}"));
        }
        Some(2)
    }
    // Tier-1 #2b — local addressing-mode fold. `Lea(t, Local(off))` whose SOLE use is the
    // very next Load/Store folds the frame offset into the memory operand:
    //   Lea(t, Local(off)) · Load(d, ty, t)   →  ldr* Rd, [sp,#pos]
    //   Lea(t, Local(off)) · Store(ty, t, v)  →  str* Rv, [sp,#pos]
    // deleting BOTH the `sub xN, x29, #off` address computation AND the address temp — the
    // dominant instruction on every local access (sqlite: ~½ the stream). pos = sp_slot_sz
    // re-bases x29−off to sp+pos: sp = x29 − frame_total ⟹ sp + (frame_total − off) = x29 −
    // off, the IDENTICAL effective address (machine translation-validation; opt-parity 0
    // DIVERGE confirms). Guards: `use_count[t]==1` (the Lea is dead after the fold), a
    // simple-GP width (exotic loads/stores keep the funnel), and a foldable pos (else the
    // eager lea form stays). Returns Some(2) on a fold, else None. Requires sp at its fixed
    // base — sp_slot_sz refuses under VLA or mid-marshalling.
    fn try_fuse_local(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
        let Inst::Lea(t, Place::Local(off)) = &insts[i] else {
            return None;
        };
        if self.use_count.get(*t as usize).copied().unwrap_or(0) != 1 {
            return None;
        }
        match insts.get(i + 1)? {
            Inst::Load(d, lty, Val::Tmp(la)) if la == t && self.simple_gp_load_ty(*lty) => {
                // Prefer the positive scaled sp form (reaches 32 KB); fall back to the x29
                // unscaled form for a small offset in a frame too large for sp-scaling; only
                // then keep the eager lea. Decide BEFORE any emission.
                let pos = self.sp_slot_sz(*off, self.a.tt.size(*lty));
                if pos.is_none() && *off > 256 {
                    return None;
                }
                let rd = self.gp_home(*d).unwrap_or(self.fnl);
                match pos {
                    Some(p) => self.load_gp_sp(rd, p, *lty),
                    None => self.load_gp_fp(rd, *off, *lty),
                }
                if self.gp_home(*d).is_none() {
                    self.tmp_store(*d, &format!("x{}", self.fnl));
                }
                Some(2)
            }
            Inst::Store(sty, Val::Tmp(la), v) if la == t && self.simple_gp_store_ty(*sty) => {
                let pos = self.sp_slot_sz(*off, self.a.tt.size(*sty));
                if pos.is_none() && *off > 256 {
                    return None;
                }
                // ISA: constant 0 stores via the zero register (str wzr/xzr) — reg 31.
                let rv = if matches!(v, Val::Imm(0)) { 31 } else { self.src_gp(*v, self.fnl) };
                match pos {
                    Some(p) => self.store_gp_sp(rv, p, *sty),
                    None => self.store_gp_fp(rv, *off, *sty),
                }
                Some(2)
            }
            _ => None,
        }
    }
    // store x{reg} → [x1] per type. MUST NOT clobber x{reg}: the value may be a live home
    // (compute-from-home path passes v's home register), so any transformation of the stored
    // value uses a scratch (x9) rather than writing back into x{reg}.
    // Store constant 0 via the zero register (caller guarantees simple_gp_store_ty).
    fn store_z(&mut self, t: TypeId) {
        let ad = self.fnl + 1; // funnel address (x1/x11)
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb wzr, [x{ad}]"),
            2 => writeln!(self.s, "\tstrh wzr, [x{ad}]"),
            4 => writeln!(self.s, "\tstr wzr, [x{ad}]"),
            _ => writeln!(self.s, "\tstr xzr, [x{ad}]"),
        };
    }
    fn store(&mut self, reg: u32, t: TypeId) {
        // Funnel address in x{ad} (base-relative); bitfield RMW scratch in x{s3}/x{s4}/x{s5}.
        // x{reg} (the stored value) is passed in and MUST NOT be clobbered — it may be a live
        // home. w9/d7/s7/q0 are fixed scratch outside every home budget.
        let (ad, s3, s4, s5) = (self.fnl + 1, self.fnl + 3, self.fnl + 4, self.fnl + 5);
        match self.a.tt.tys[t as usize] {
            Ty::Bool => {
                _ = writeln!(
                    self.s,
                    "\tcmp x{reg}, #0\n\tcset w9, ne\n\tstrb w9, [x{ad}]"
                );
            }
            Ty::Float => {
                _ = writeln!(self.s, "\tfmov d7, x{reg}\n\tfcvt s7, d7\n\tstr s7, [x{ad}]");
            }
            Ty::LDouble => {
                // bl clobbers x1 (caller-saved) — shield the address via the stack. LDouble Store
                // forces NARROW (heavy scan) ⟹ ad=1 here.
                _ = writeln!(
                    self.s,
                    "\tstr x{ad}, [sp, #-16]!\n\tfmov d0, x{reg}\n\tbl __extenddftf2\n\tldr x{ad}, [sp], #16\n\tstr q0, [x{ad}]"
                );
            }
            Ty::Bitfield(b, boff, w) => {
                // read-modify-write the containing unit, field-insert via ARMv8 BFI [Phase 5.2].
                // `bfi rD,rS,#lsb,#width` (BFM alias, ARM DDI0487 C6.2.34) sets rD<lsb+width-1:lsb>
                // = rS<width-1:0> and leaves every other bit of rD unchanged — EXACTLY the RMW this
                // used to spell out (materialize mask ; `bic` clear field ; `lsl`+`and` place value's
                // low `w` bits ; `orr` insert). Translation-validation: pure ISA identity — the old
                // `and x{s5},x{s5},mask` kept only rS<w-1:0> at [boff,boff+w), which is bfi's
                // definition. Register form follows the container width (w-form ⟹ boff+w≤32 by
                // layout; usz=8 ⟹ x-form). Collapses 7–9 insns → 3. s4/s5 scratch now unused.
                let _ = (s4, s5);
                let (usz, rw) = (self.a.tt.size(b), if self.a.tt.size(b) == 8 { 'x' } else { 'w' });
                _ = match usz {
                    1 => writeln!(self.s, "\tldrb w{s3}, [x{ad}]"),
                    2 => writeln!(self.s, "\tldrh w{s3}, [x{ad}]"),
                    4 => writeln!(self.s, "\tldr w{s3}, [x{ad}]"),
                    _ => writeln!(self.s, "\tldr x{s3}, [x{ad}]"),
                };
                _ = writeln!(self.s, "\tbfi {rw}{s3}, {rw}{reg}, #{boff}, #{w}");
                _ = match usz {
                    1 => writeln!(self.s, "\tstrb w{s3}, [x{ad}]"),
                    2 => writeln!(self.s, "\tstrh w{s3}, [x{ad}]"),
                    4 => writeln!(self.s, "\tstr w{s3}, [x{ad}]"),
                    _ => writeln!(self.s, "\tstr x{s3}, [x{ad}]"),
                };
            }
            _ => {
                _ = match self.a.tt.size(t) {
                    1 => writeln!(self.s, "\tstrb w{reg}, [x{ad}]"),
                    2 => writeln!(self.s, "\tstrh w{reg}, [x{ad}]"),
                    4 => writeln!(self.s, "\tstr w{reg}, [x{ad}]"),
                    _ => writeln!(self.s, "\tstr x{reg}, [x{ad}]"),
                };
            }
        }
    }
    // Copy `sz` bytes: src (x0) → dst (x1), forward. Leaves the dst address in x0 (the
    // rvalue of a struct assignment = the destination address). Shared: AST-path + IR Inst::Memcpy.
    fn blk_copy(&mut self, sz: u32) {
        // Funnel (base-relative): src = x{s} (x0/x10), dst = x{d} (x1/x11), count = x{c},
        // byte = w{by}, saved-dst = x{sv}.
        let (s, d, c, by, sv) = (self.fnl, self.fnl + 1, self.fnl + 2, self.fnl + 3, self.fnl + 4);
        _ = writeln!(self.s, "\tmov x{sv}, x{d}");
        if sz > 0 {
            let n = self.labels(1);
            self.imm(&format!("x{c}"), sz as i64);
            _ = writeln!(self.s, "L{n}:");
            _ = writeln!(self.s, "\tldrb w{by}, [x{s}], #1\n\tstrb w{by}, [x{d}], #1\n\tsubs x{c}, x{c}, #1");
            _ = writeln!(self.s, "\tb.ne L{n}");
        }
        _ = writeln!(self.s, "\tmov x{s}, x{sv}"); // value = dst address
    }
    // Convert the canonical value in the funnel register x{fnl} (x0/x10): from → to. d0/s0 are
    // FP scratch (never homes); the GP carrier is base-relative.
    fn cast_op(&mut self, from: TypeId, to: TypeId) {
        let v = self.fnl;
        let tt = &self.a.tt;
        if matches!(
            tt.tys[to as usize],
            Ty::Void | Ty::Struct(_) | Ty::Array(..)
        ) {
            return;
        }
        match (tt.is_float(from), tt.is_float(to)) {
            (false, false) => self.ext_r(v, to),
            (false, true) => {
                let cvt = if tt.is_unsigned(from) {
                    "ucvtf"
                } else {
                    "scvtf"
                };
                // int32 value contract (see ir_bin_r): a <8-byte int lives in w-form — its high
                // 32 bits are DON'T-CARE, NOT sign-extended. Convert from the SOURCE-width
                // register: `scvtf d0, w{v}` reads the low 32 with the correct sign, whereas
                // `scvtf d0, x{v}` would convert the garbage high bits too (proven: torture
                // pr59643's `(double)((i&7)-4)` turned −4 into 4294967292.0). 8-byte stays x{v}.
                let sr = if tt.size(from) < 8 { "w" } else { "x" };
                _ = writeln!(self.s, "\t{cvt} d0, {sr}{v}");
                if tt.size(to) == 4 {
                    self.s += "\tfcvt s0, d0\n\tfcvt d0, s0\n";
                }
                _ = writeln!(self.s, "\tfmov x{v}, d0");
            }
            (true, false) => {
                if matches!(tt.tys[to as usize], Ty::Bool) {
                    _ = writeln!(self.s, "\tfmov d0, x{v}\n\tfcmp d0, #0.0\n\tcset x{v}, ne");
                    return;
                }
                _ = writeln!(self.s, "\tfmov d0, x{v}");
                let cvt = if self.a.tt.is_unsigned(to) {
                    "fcvtzu"
                } else {
                    "fcvtzs"
                };
                if self.a.tt.size(to) == 8 {
                    _ = writeln!(self.s, "\t{cvt} x{v}, d0");
                } else {
                    _ = writeln!(self.s, "\t{cvt} w{v}, d0");
                    self.ext_r(v, to);
                }
            }
            (true, true) => {
                if tt.size(to) == 4 {
                    _ = writeln!(self.s, "\tfmov d0, x{v}\n\tfcvt s0, d0\n\tfcvt d0, s0\n\tfmov x{v}, d0");
                }
            }
        }
    }
    // Function address → x0. Shared by the AST-walk (Node::FunAddr) and IR (Inst::FunAddr)
    // → BYTE-IDENTICAL asm. A static function is a LOCAL symbol: it must NOT go through the
    // GOT — gas lowers a local relocation to .text+addend, and GNU ld creates a GOT entry
    // that DROPS the addend → the pointer points to the wrong function at the start of the
    // section (musl libc_start_main_stage2 → jumps into __syscall3). Local within the same
    // TU → adrp/add directly.
    fn emit_funaddr(&mut self, name: &str) {
        let v = self.fnl; // funnel result register (x0/x10)
        let sy = sym(name);
        if self.a.funcs.iter().any(|f| f.name == name && f.is_static) {
            _ = writeln!(self.s, "\tadrp x{v}, {0}\n\tadd x{v}, x{v}, :lo12:{0}", sy);
        } else {
            _ = writeln!(self.s, "\tadrp x{v}, :got:{0}\n\tldr x{v}, [x{v}, :got_lo12:{0}]", sy);
        }
    }
    // memset(x{fnl}, 0, sz): zero sz bytes starting at the funnel address. Shared by the
    // AST-walk (Node::Zero) and IR (Inst::Zero). sz==0 → no-op.
    fn emit_zero(&mut self, sz: u32) {
        if sz == 0 {
            return;
        }
        let (ad, c) = (self.fnl, self.fnl + 2); // address (x0/x10), count scratch (x2/x12)
        self.imm(&format!("x{c}"), sz as i64);
        let n = self.labels(1);
        _ = writeln!(self.s, "L{n}:");
        _ = writeln!(self.s, "\tstrb wzr, [x{ad}], #1\n\tsubs x{c}, x{c}, #1");
        _ = writeln!(self.s, "\tb.ne L{n}");
    }
    // EXT(gcc): &&label (computed-goto) → x{fnl}. A local label within the current function.
    fn emit_labeladdr(&mut self, name: &str) {
        let v = self.fnl;
        _ = writeln!(
            self.s,
            "\tadrp x{v}, lg_{0}.{1}\n\tadd x{v}, x{v}, :lo12:lg_{0}.{1}",
            self.fname, name
        );
    }
    // __builtin_*_overflow: x0=a, x1=b, x9=&res. bool result → x0. Shared by the
    // AST-walk (Node::Overflow) + IR (Inst::Overflow). ta/tb/rt = types of a/b/*rp.
    fn emit_overflow(&mut self, op: u8, ta: TypeId, tb: TypeId, rt: TypeId) {
        let a_sg = !self.a.tt.is_unsigned(ta);
        let b_sg = !self.a.tt.is_unsigned(tb);
        let (r_sg, rw) = (!self.a.tt.is_unsigned(rt), self.a.tt.size(rt));
        // int32 value contract (see ir_bin_r): a <8-byte operand arrives in w-form with its
        // high 32 bits DON'T-CARE. overflow_emit embeds each operand as a 128-bit two's-
        // complement value by reading the FULL x0/x1 and sign/zero-extending from bit 63
        // (`asr xhi,x,#63` / `mov xhi,#0`) — that embedding is only correct if x0/x1 already
        // hold the canonical-64 form. Canonicalize here (sxtw for signed, `mov w` for
        // unsigned) so the 128-bit product is faithful (proven: torture pr84169's
        // `mul_overflow((unsigned char)h, -16, …)` turned −64 into (4<<32)−64).
        if self.a.tt.size(ta) < 8 {
            self.ext_rd(0, 0, ta);
        }
        if self.a.tt.size(tb) < 8 {
            self.ext_rd(1, 1, tb);
        }
        crate::ext::overflow_emit(&mut self.s, op, a_sg, b_sg, r_sg, rw);
    }
    // va_start: x0 = &ap. Fill the AAPCS va_list from the prologue state (va=gp,fp,stk,frame).
    // Shared by the AST-walk (Node::VaStart) + IR (Inst::VaStart).
    fn emit_vastart(&mut self) {
        let ap = self.fnl; // &ap funnel address (x0/x10); x9 is the fixed value scratch (not a home)
        let (gp, fp, stk, frame) = self.va;
        self.imm("x9", (16 + stk) as i64);
        _ = writeln!(self.s, "\tadd x9, x29, x9\n\tstr x9, [x{ap}]"); // __stack
        self.imm("x9", frame as i64);
        _ = writeln!(self.s, "\tsub x9, x29, x9\n\tstr x9, [x{ap}, #8]"); // __gr_top
        _ = writeln!(self.s, "\tsub x9, x9, #64\n\tstr x9, [x{ap}, #16]"); // __vr_top
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x{ap}, #24]", (gp as i64 - 8) * 8);
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x{ap}, #28]", (fp as i64 - 8) * 16);
    }
    // va_arg(*(&ap) in x0, type t, scratch-local tmp for HFA gather) → result in x0.
    // Shared by the AST-walk (Node::VaArg) + IR (Inst::VaArg). AAPCS details below.
    fn emit_vaarg(&mut self, t: TypeId, tmp: u32) {
        let st = matches!(self.a.tt.tys[t as usize], Ty::Struct(_));
        let sz = self.a.tt.size(t);
        let fl = self.a.tt.is_float(t);
        let hfa = if st { self.a.tt.hfa(t) } else { None };
        let (offs, top, step) = if fl {
            (28, 16, 16)
        } else if let Some((_, n)) = hfa {
            (28, 16, n * 16)
        } else if st && sz <= 16 {
            (24, 8, sz.div_ceil(8) * 8)
        } else {
            (24, 8, 8)
        };
        let ldbl = matches!(self.a.tt.tys[t as usize], Ty::LDouble);
        let stk_step = if st && (sz <= 16 || hfa.is_some()) {
            sz.div_ceil(8) * 8
        } else if ldbl {
            16
        } else {
            8
        };
        // AAPCS (GCC PR92904): a composite by-value is NOT split across reg/stack —
        // consume offs first, going to a register only when the NEW offs ≤ 0; crossing 0
        // → the whole block falls to the stack.
        let blk = st && (sz <= 16 || hfa.is_some());
        let l = self.labels(2);
        _ = writeln!(self.s, "\tldr w9, [x0, #{offs}]");
        if blk {
            _ = writeln!(self.s, "\tadd w10, w9, #{step}\n\tstr w10, [x0, #{offs}]");
            _ = writeln!(self.s, "\tcmp w10, #0\n\tb.le L{l}");
        } else {
            _ = writeln!(self.s, "\ttbnz w9, #31, L{l}");
        }
        self.s += "\tldr x10, [x0]\n";
        if ldbl {
            self.s += "\tadd x10, x10, #15\n\tand x10, x10, #0xfffffffffffffff0\n";
        }
        self.s += "\tadd x11, x10, #";
        _ = writeln!(self.s, "{}\n\tstr x11, [x0]\n\tb L{}", stk_step, l + 1);
        _ = writeln!(self.s, "L{l}:\n\tldr x10, [x0, #{top}]\n\tadd x10, x10, w9, sxtw");
        if !blk {
            _ = writeln!(self.s, "\tadd w9, w9, #{step}\n\tstr w9, [x0, #{offs}]");
        }
        if let Some((dbl, n)) = hfa {
            self.lea_local("x11", tmp);
            for j in 0..n {
                if dbl {
                    _ = writeln!(self.s, "\tldr x12, [x10, #{}]", 16 * j);
                    _ = writeln!(self.s, "\tstr x12, [x11, #{}]", 8 * j);
                } else {
                    _ = writeln!(self.s, "\tldr w12, [x10, #{}]", 16 * j);
                    _ = writeln!(self.s, "\tstr w12, [x11, #{}]", 4 * j);
                }
            }
            self.s += "\tmov x10, x11\n";
        }
        _ = writeln!(self.s, "L{}:\n\tmov x0, x10", l + 1);
        if st {
            if sz > 16 && hfa.is_none() {
                self.s += "\tldr x0, [x0]\n"; // >16B: the slot holds a POINTER
            } // struct: value = address (zcc's struct-expression convention)
        } else {
            self.load(t);
        }
    }

}

// EXT(gcc): symbol emit — a \x01 prefix (asm-label / && label) = name already complete; ELF has no '_' prefix
fn sym(n: &str) -> String {
    match n.strip_prefix('\x01') {
        Some(raw) => raw.to_string(),
        None => n.to_string(),
    }
}

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
    fn ir_toff(&self, i: Tmp) -> u32 {
        self.ir_tbase + 8 + self.spill_off[i as usize]
    }
    // GP color → physical register, per the ACTIVE budget for this function (§3). WIDE opens
    // 6 caller-saved homes x10–x15 (colors 0..6) ahead of the callee-saved x19–x28; NARROW is
    // the callee-only file. `gp_ncaller()` reports the split so csave / verify agree.
    fn gpp(&self, idx: u32) -> u32 {
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
    fn gp_ncaller(&self) -> u32 {
        if self.gp_wide { GP_BUDGET_WIDE.ncaller } else { GP_BUDGET.ncaller }
    }
    // Stage 5b — a temp's home is a physical register (Chaitin color) or a spill slot.
    // `reg` is always a 64-bit GPR (verified: every call site passes an x-form); an
    // FP-homed temp holds the f64 bit pattern (SEMANTICS §1), moved via `fmov` GPR↔d-reg.
    fn tmp_load(&mut self, i: Tmp, reg: &str) {
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
    fn tmp_store(&mut self, i: Tmp, reg: &str) {
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
    fn save_callee(&mut self, store: bool) {
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
    fn ld_val(&mut self, v: Val, reg: &str) {
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
    fn src_gp(&mut self, v: Val, scratch: u32) -> u32 {
        if let Val::Tmp(t) = v {
            if let Some((false, idx)) = self.talloc.get(t as usize).copied().flatten() {
                return self.gpp(idx);
            }
        }
        self.ld_val(v, &format!("x{scratch}"));
        scratch
    }
    // The GP home register of temp `d` if it is GP-register-resident, else None (spilled).
    fn gp_home(&self, d: Tmp) -> Option<u32> {
        match self.talloc.get(d as usize).copied().flatten() {
            Some((false, idx)) => Some(self.gpp(idx)),
            _ => None,
        }
    }
    fn ir_label(&self, b: u32) -> String {
        format!(".Lir_{}_{}", self.fname, b)
    }
    fn val_is_float(&self, v: Val) -> bool {
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
    fn lea_global(&mut self, reg: u32, name: &str, off: i64) {
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
    fn ir_bin(&mut self, op: Op, ct: TypeId) {
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
    fn ir_bin_r(&mut self, op: Op, ct: TypeId, rd: u32, ra: u32, rb: u32) {
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
    fn try_bin_imm(&mut self, op: Op, ct: TypeId, rd: u32, ra: u32, b: Val) -> bool {
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
    fn ir_ret_conv(&mut self) {
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
    fn marshal_call_args(&mut self, args: &[Val]) {
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

    fn ir_call(&mut self, dst: &Option<Tmp>, callee: &Callee, args: &[Val]) {
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
    fn ir_call_abi(
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
    fn ir_sync(&mut self, dst: &Option<Tmp>, op: SyncOp, operands: &[Val], sz: u32, ret: TypeId) {
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
    fn ir_asm(&mut self, tpl: &str, ops: &[crate::ir::AsmIrOp]) {
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

    fn emit_inst(&mut self, i: &Inst) {
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
    fn cbr_relational(&self, blk: &crate::ir::Block) -> Option<(Op, TypeId, Val, Val)> {
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
    fn emit_cbr(&mut self, op: Op, ct: TypeId, a: Val, b: Val, tb: u32, eb: u32, ft: Option<u32>) {
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

    fn emit_term(&mut self, t: &Term, ft: Option<u32>) {
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
                if ft == Some(*eb) {
                    // else-edge falls through: c==0 → next block (jump-to-adjacent = fall),
                    // c!=0 → `b then`. cbz targets the adjacent L{eb} (in range); 2 insns.
                    _ = writeln!(self.s, "\tcbz x{rc}, {le}\n\tb {lt}");
                } else if ft == Some(*tb) {
                    // then-edge falls through: c!=0 → next block, c==0 → `b else`. cbnz targets
                    // the adjacent L{tb} (in range); 2 insns.
                    _ = writeln!(self.s, "\tcbnz x{rc}, {lt}\n\tb {le}");
                } else if self.near_branch {
                    // Small function: every intra-function label is within cb(n)z's ±1MB
                    // imm19 reach, so branch straight to `then` and `b` the far else. 2 insns.
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
    fn compute_ext_folds(&self, irf: &IrFunc) -> (std::collections::HashMap<Tmp, ExtFold>, std::collections::HashSet<Tmp>) {
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
    fn compute_imm_folds(&self, irf: &IrFunc) -> (std::collections::HashMap<Tmp, (u32, u32)>, std::collections::HashSet<Tmp>) {
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

    fn emit_ir_body(&mut self, irf: &IrFunc) {
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


mod peephole;
use peephole::*;
// ─────────────────────────────────────────────────────────────────────────────
// COST-SQUARE — the Law-3 cost-theorem instrument (dual of the correctness commuting
// square). Correctness proves ⟦f⟧=⟦opt(f)⟧; cost proves the instruction-count is what the
// lowering theorems SAY it is, at the theorem layer — not grepped out of the compiled `.s`
// after a slow patch→build→suite cycle. The final emitted count decomposes EXACTLY:
//
//     FINAL(f) = LOWER(f) − Δload(f) − Δmove(f) − Δpair(f) − Δthread(f)
//
// where LOWER is the pre-peephole lowering catamorphism (Σ over IR insts of their machine-
// insn expansion) and each Δ is the reduction realized by one post-emit theorem. The four Δ
// are string→string passes, so their contribution is FAITHFUL BY CONSTRUCTION — measured by
// counting instruction lines before/after. LOWER is the one layer needing an independent
// model (`emit_len`, Layer 1): the square `Σ emit_len(f) ≡ LOWER(f)` certifies the Rust in
// `emit_inst` faithfully realizes its lowering theorem; a mismatch is a Law-2 defect localized
// to one function. This census is Layer 0: the exact per-stage decomposition, gated on
// ZCC_COSTSQUARE, printed to stderr — deterministic, in-process, no `.s` grep.
// ─────────────────────────────────────────────────────────────────────────────

/// Count EMITTED MACHINE INSTRUCTIONS in an assembly fragment: exactly the lines beginning
/// with `\t` + a lowercase mnemonic letter. Labels (`.L…:`, `lg_…:` — no leading tab) and
/// directives (`\t.cfi…`, `\t.size` — the byte after `\t` is `.`, not a-z) are excluded. This
/// is the ground-truth `len(·)` the whole cost-model is built on.
fn count_insn_lines(s: &str) -> usize {
    s.lines()
        .filter(|l| {
            let b = l.as_bytes();
            b.first() == Some(&b'\t') && b.get(1).is_some_and(|c| c.is_ascii_lowercase())
        })
        .count()
}

/// Print the per-stage cost-square census (ZCC_COSTSQUARE). `log[i] = (fname, [LOWER,
/// after-load, after-move, after-pair, FINAL])` — five cumulative snapshots; the theorem Δ are
/// the successive differences. Aggregate first (the TU-wide decomposition), then the top
/// functions by FINAL size — where residual bloat concentrates, i.e. the Law-4 targets.
fn report_cost_square(log: &[(String, [usize; 5])]) {
    let mut t = [0usize; 5];
    for (_, c) in log {
        for i in 0..5 {
            t[i] += c[i];
        }
    }
    let [lower, a_load, a_move, a_pair, fin] = t;
    eprintln!("── COST-SQUARE census ({} funcs) ─────────────────────", log.len());
    eprintln!("  LOWER  (Σ lowering)   {lower:>9}   ← Layer-1 target: Σ emit_len must equal this");
    eprintln!("  − load-elim           {:>9}   (store→load identity)", lower - a_load);
    eprintln!("  − move-peephole       {:>9}   (redundant/dead reg-mov)", a_load - a_move);
    eprintln!("  − ldp/stp pair        {:>9}", a_move - a_pair);
    eprintln!("  − branch-thread       {:>9}", a_pair - fin);
    eprintln!("  FINAL  (emitted)      {fin:>9}");
    let mut idx: Vec<usize> = (0..log.len()).collect();
    idx.sort_by(|&a, &b| log[b].1[4].cmp(&log[a].1[4]));
    eprintln!("  ── top-15 by FINAL ──          LOWER  -LOAD  -MOVE  -PAIR  -THRD    FINAL");
    for &i in idx.iter().take(15) {
        let (n, c) = &log[i];
        let nm: String = n.chars().take(26).collect();
        eprintln!(
            "    {:<26} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8}",
            nm, c[0], c[0] - c[1], c[1] - c[2], c[2] - c[3], c[3] - c[4], c[4]
        );
    }
}
/// Backend entry point — the SOLE path: lower(AST) → IR → passes → asm. Covers the
/// full suite/csmith/musl; the AST-walk emit() has been removed. The backend simulates per-inst.
pub fn emit_ir(ast: &Ast) -> String {
    let mut funcs = ir::lower(ast);
    // Optimization is DEFAULT-ON on this branch (the ssa-qbe fork IS the optimizing
    // compiler): optimize_ssa = to_ssa ▸ sccp/gvn/const-fold/copy-prop/cse/dce ▸ out_of_ssa,
    // returning φ-free IR the naive-slot backend consumes unchanged. Every pass is proven
    // ⟦·⟧-preserving (opt.rs::tests, commuting-square); verify rejects broken IR.
    // Two guards, both MANDATORY (not A/B scaffolding):
    //   (1) volatile — the IR does not model volatile (6.7.3), so ⟦·⟧-preservation is proven
    //       only for volatile-free code. Gated PER FUNCTION on `Func::has_volatile`, which the
    //       parser now computes TYPE-accurately: true ⟺ some lvalue in the function has a
    //       volatile-qualified type (TyTab::vol rides the TypeId from the decl/typedef/pointee/
    //       member to the access node). That flagged function keeps the naive -O0 path while its
    //       volatile-free peers optimize — no whole-TU fallback, because a volatile file-scope
    //       object reached here is read through a volatile-typed node and so flags THIS function
    //       directly (the volatile-typedef-used-in-a-function case a token scan missed).
    //   (2) ZCC_O0 — the -O0 escape (debug + the bench baseline), the sole knob that turns
    //       the optimizer off. Default (unset) = full SSA optimization + regalloc.
    let zcc_o0 = std::env::var("ZCC_O0").is_ok();
    // funcs[i] ↔ ast.funcs[i] by construction (ir::lower pushes one per AST func, in order).
    // opt_ok now gates ONLY on ZCC_O0: the IR passes are volatile-SAFE per access (each pins
    // volatile Load/Store via TyTab::vol — is_volatile_access — so a volatile access is never
    // removed/merged/duplicated/hoisted/promoted), so a function with a volatile access is
    // FULLY optimized (regalloc + SSA passes) while only its volatile accesses stay memory-bound.
    // This replaces the old whole-function -O0 fallback that de-optimized giant functions
    // (sqlite3VdbeExec: one volatile `db->u1.isInterrupted` read all-spilled 14,522 temps).
    // `vol_free` remains for the two places that still need whole-function volatile caution: the
    // inline callee gate (a volatile callee is not spliced), and the backend `[sp,#k]` memory
    // peepholes (which cannot tell a volatile local slot from a compiler temp in text).
    // `has_vla` → -O0: a VLA function has a DYNAMIC frame (sp moves on each VLA alloc, and a
    // backward goto to a label before the VLA def must deallocate via reset_sp_base). The SSA
    // passes + regalloc do not yet model that SP dance — an optimized VLA function with a
    // backward goto SEGFAULTS (gcc.c-torture vla-dealloc-1, reduced: a non-volatile VLA+goto also
    // crashes under opt). This bug was MASKED because the suite's only VLA-dealloc test carries a
    // `volatile` (→ old -O0 gate); enabling volatile optimization exposed it. Routing VLA to -O0
    // is sound and rare (no VLA in sqlite's hot path); VLA+opt is TRACKED DEBT (OPT.md).
    let opt_ok: Vec<bool> = ast.funcs.iter().map(|f| !zcc_o0 && !f.has_vla).collect();
    let vol_free: Vec<bool> = ast.funcs.iter().map(|f| !zcc_o0 && !f.has_volatile).collect();
    // Industrial toggleable pipeline: which passes run is read once from the environment
    // (ZCC_OPT_OFF / ZCC_OPT_ON over the default profile). `coalesce` is consumed later by
    // abi_alloc, so it is stashed on Cg.
    let passes = crate::opt::Passes::from_env();
    {
        // Tier-1 #5: whole-program inlining runs FIRST (it is interprocedural — it reads
        // the whole `funcs` set), on straight-lowered IR; the per-function SSA passes then
        // clean up the spliced copies + β-substitute across the inline (const-prop, DCE).
        if passes.inline {
            // A variadic / VLA caller must not be inlined INTO: the callee frame is appended
            // into its reg-save / dynamic-SP region (see opt::inline). Both caller AND callee
            // must be opt-eligible (a volatile function is never optimized, nor spliced into
            // optimized code).
            let caller_ok: Vec<bool> = ast
                .funcs
                .iter()
                .enumerate()
                .map(|(i, f)| opt_ok[i] && !f.variadic && !f.has_vla)
                .collect();
            crate::opt::inline(&ast.tt, &mut funcs, &caller_ok, &vol_free, &crate::opt::InlineCfg::from_env());
        }
        for (i, f) in funcs.iter_mut().enumerate() {
            if opt_ok.get(i).copied().unwrap_or(false) {
                // NARROW k=10 (not WIDE's 16) on purpose: the wide/narrow choice is made per
                // function LATER, at emit time (self.gp_wide, line ~2430), from `heavy` — not yet
                // known here. Passing the SMALLER budget makes licm/SR's pressure guard a
                // conservative LOWER bound: k=10 ≤ whatever the allocator ends up with, so the
                // guard can only under-hoist, never over-pressure. Correctness is k-independent
                // (opt.rs:2326); size impact ≈ 0 (hoists are size-neutral).
                crate::opt::optimize_ssa(&ast.tt, f, &passes, GP_BUDGET.k);
                debug_assert!(ir::verify(f).is_ok(), "opt produced broken IR: {}", f.name);
            }
        }
    }
    // Dead-function elimination — after inline, a static function whose every call site was
    // spliced away (and whose address is not taken anywhere reachable) is unreferenced ⟹ its
    // standalone body is dead code (gcc's remove-unused-static). Roots = exported (non-static)
    // functions + every symbol NAMED by a global initializer (function-pointer tables — sqlite's
    // vtab/opcode method arrays). Reachability follows Call/CallX/FunAddr edges to a fixpoint.
    let dead: Vec<bool> = if passes.inline {
        let mut root_syms: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stack: Vec<&crate::ast::GInit> = ast.globals.iter().map(|g| &g.init).collect();
        while let Some(init) = stack.pop() {
            match init {
                crate::ast::GInit::Addr(n, _) => {
                    root_syms.insert(n.strip_prefix('\x01').unwrap_or(n).to_string());
                    root_syms.insert(n.clone());
                }
                crate::ast::GInit::Diff(a, b) => {
                    root_syms.insert(a.clone());
                    root_syms.insert(b.clone());
                }
                crate::ast::GInit::List(items) => {
                    for (_, _, sub) in items {
                        stack.push(sub);
                    }
                }
                _ => {}
            }
        }
        let is_static: Vec<bool> = ast.funcs.iter().map(|f| f.is_static).collect();
        crate::opt::dead_static_fns(&funcs, &is_static, &root_syms)
    } else {
        vec![false; funcs.len()]
    };
    // Stage 5b — ABI-aware regalloc runs per function whenever that function is optimized
    // (φ-free, volatile-free IR); off ⟹ the naive all-spill memory model (the -O0 baseline).
    // Set on `g` inside the emit loop from `opt_ok[fi]`.
    let mut g = Cg {
        s: String::from(".cfi_sections .eh_frame\n.text\n"),
        a: ast,
        lbl: 0,
        fname: String::new(),
        fret: VOID,
        fsret: 0,
        va: (0, 0, 0, 0),
        fframe: 0,
        fvariadic: false,
        fhasvla: false,
        ir_tbase: 0,
        ir_temps: Vec::new(),
        ir_tspill: 0,
        regalloc: false, // set per function from opt_ok[fi] in the emit loop
        coalesce: passes.coalesce,
        talloc: Vec::new(),
        spill_off: Vec::new(),
        gp_wide: false,
        fnl: 0,
        csave_gp: Vec::new(),
        csave_fp: Vec::new(),
        use_count: Vec::new(),
        ext_fold: std::collections::HashMap::new(),
        ext_skip: std::collections::HashSet::new(),
        imm_fold: std::collections::HashMap::new(),
        sp_at_base: true,
        fdynstack: false,
        near_branch: false,
        param_loc: Vec::new(),
        param_ref: std::collections::HashSet::new(),
    };
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n";
    }
    // Cost-square census (Layer 0): five cumulative instruction-line snapshots per function.
    let costsq = std::env::var("ZCC_COSTSQUARE").is_ok();
    let mut cost_log: Vec<(String, [usize; 5])> = Vec::new();
    for (fi, f) in ast.funcs.iter().enumerate() {
        if dead.get(fi).copied().unwrap_or(false) {
            continue; // dead-function elimination: unreferenced static, fully inlined away
        }
        g.regalloc = opt_ok[fi]; // per-function: optimized ⟺ volatile-free (C99 6.7.3)
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
        // Use the IR func's frame (line 1100: lowered == AST pre-inline), which INLINING
        // may have GROWN by the appended callee frames. Every frame-derived offset (the
        // C-frame SP reservation below, the temp base ir_tbase, the va-save region, the
        // VLA reset base) must read this ONE grown value or the temp/callee-save slab
        // lands below sp → stack corruption (the inline segfault: gcc pr40668).
        g.fframe = funcs[fi].frame;
        g.fvariadic = f.variadic;
        g.fhasvla = f.has_vla;
        let fn_start = g.s.len(); // start of THIS function's text (for the post-emit sp-fuse)
        if !f.is_static {
            _ = writeln!(g.s, ".globl {}", f.name);
            if f.is_inline || f.is_weak {
                _ = writeln!(g.s, ".weak {}", f.name);
            }
        }
        _ = writeln!(g.s, ".type {}, %function", f.name);
        _ = write!(
            g.s,
            ".p2align 2\n{}:\n\t.cfi_startproc\n\tstp x29, x30, [sp, #-16]!\n\t.cfi_def_cfa_offset 16\n\t.cfi_offset 29, -16\n\t.cfi_offset 30, -8\n\tmov x29, sp\n\t.cfi_def_cfa_register 29\n",
            f.name
        );
        if g.fframe > 0 {
            g.sp_adjust("sub", g.fframe);
        }
        // Step-2 param promotion (regalloc only): a scalar param whose slot has NO IR
        // Lea(Local(off)) was promoted to Inst::Param (to_ssa) — emit_params must SKIP its
        // spill (Param delivers the arg register into the home instead). Build the referenced
        // set from the IR; size param_loc (filled by emit_params' ABI walk). At -O0 (no
        // regalloc) param_ref is left empty ⟹ every param spills, the pre-Step-2 behavior.
        g.param_loc = vec![ParamLoc::None; f.params.len()];
        g.param_ref.clear();
        if g.regalloc {
            for b in &funcs[fi].blocks {
                for i in &b.insts {
                    if let Inst::Lea(_, crate::ir::Place::Local(off)) = i {
                        g.param_ref.insert(*off);
                    }
                }
            }
        } else {
            // -O0: no Inst::Param is produced, so every param slot must be spilled. Seeding
            // param_ref with all param offsets keeps emit_params on its unconditional path.
            g.param_ref = f.params.iter().map(|&(off, _)| off).collect();
        }
        // Prologue parameter-ABI SHARED with emit() (nested-chain/variadic-save/sret/
        // spill scalar+struct+HFA) → parameters sit ready in frame slots for the IR body.
        emit_params(&mut g, f);
        let body_start = g.s.len();
        g.emit_ir_body(&funcs[fi]);
        // Phase C — machine-level cleanup over just this body (the region begins fresh:
        // entered from the prologue, so an empty equivalence model is sound).
        let mut body = g.s.split_off(body_start);
        // Layer-0 snapshot #1: the pre-peephole lowering count (LOWER) — the ground truth the
        // independent emit_len catamorphism (Layer 1) is proven equal to.
        let c_lower = costsq.then(|| count_insn_lines(&body));
        // Redundant-load-after-store (store→load identity) matches only `[sp,#k]` frame slots,
        // which hold compiler temps — a program volatile object is accessed through an address
        // register (`[xN]`), never this form. That invariant holds because volatile objects live
        // only in `has_volatile` (→ -O0) functions, whose local Var accesses are not sp-folded.
        // We still gate on `!has_volatile` so the pass's soundness is LOCAL (no reliance on a
        // cross-pass fold invariant) — volatile-free functions, including at -O0, keep the win.
        if !f.has_volatile {
            body = drop_redundant_loads(&body); // run on the raw stream, before copy-prop renames
        }
        let c_load = costsq.then(|| count_insn_lines(&body));
        // GP registers the caller can observe at `ret` — the seed for cross-block dead-move
        // liveness. sret (struct via x8) returns the pointer in x0:x1 (conservative 2); a scalar
        // ≤8B → x0; a 128-bit scalar/struct → x0:x1; void/float/HFA → none (result in v-regs).
        // Live-out at every `ret`: exactly the caller-visible return register(s), as a u64 mask
        // over the tracked register space (GP x0.. ↔ bits 0.., FP d0.. ↔ bits 32..). sret →
        // pointer in x0; void → none; HFA → d0..d(n-1); scalar float → d0; >8B int → x0:x1;
        // else → x0. Seeds `drop_dead_moves`' backward liveness (its only entry into the function).
        let exit_live: u64 = if f.sret != 0 {
            0b1
        } else if matches!(g.a.tt.tys[f.ret as usize], Ty::Void) {
            0
        } else if let Some((_, n)) = g.a.tt.hfa(f.ret) {
            ((1u64 << n) - 1) << 32
        } else if g.a.tt.is_float(f.ret) {
            1u64 << 32
        } else if g.a.tt.size(f.ret) > 8 {
            0b11
        } else {
            0b1
        };
        if g.regalloc && passes.peephole {
            body = peephole_moves(&body, exit_live); // redundant/dead reg-moves…
            if !f.has_volatile {
                // …then fold a loop-IV `mem [xP]; add xP,xP,#k` into a post-index access. Skipped
                // for volatile functions (the increment's hoist reorders relative to the access).
                body = post_index(&body);
            }
            // …and collapse `cmp Rn,#0; b.eq/ne` → `cbz/cbnz Rn` (pure control flow, volatile-safe).
            body = cbz_fuse(&body);
            // …and fuse a strength-reduced scaled index `lsl xT,xM,#s; add xD,xA,xT` (xD==xT)
            // into one shifted-register `add xD,xA,xM,lsl #s` (pure ISA identity, volatile-safe).
            body = fuse_shifted_arith(&body);
            // …then absorb a preceding signed-index widening `sxtw xT,wS; add xD,xB,xT[,lsl #k]`
            // (k≤4) into the add's extend field: `add xD,xB,wS,sxtw #k` (drops the sxtw + shift).
            body = fuse_sxtw_extend(&body);
            // …and drop redundant fmov round-trips (d→GP→d spill/reload) via 64-bit x+d
            // value-equivalence (Phase 4.2 FP residency; pure register identity, volatile-safe).
            body = fmov_residency(&body);
            // …then collapse an imm8-encodable double built as mov/movk/fmov-x into one
            // `fmov d, #imm` (Phase 4.1 FP-constant materialization; volatile-safe ISA identity).
            body = fold_fp_imm(&body);
            // …and collapse a d→x→d GP bridge `fmov xN,dS; fmov dD,xN` → `fmov dD,dS` (Phase 4.3;
            // the dead GP hop is reaped by the dead-def sweep below). Pure register-copy identity.
            body = collapse_fp_bridge(&body);
            // …then a SECOND dead-def sweep: residency deletes the reads that kept a write-only FP
            // home (`fmov d17,x10` never reloaded) alive, so its stores are now provably dead. The
            // in-peephole pass ran before residency, when the read still stood — re-run to reap them.
            body = drop_dead_moves(&body, exit_live);
        }
        let c_move = costsq.then(|| count_insn_lines(&body));
        if g.regalloc && passes.ldst_pair && !f.has_volatile {
            // ldp/stp merges two adjacent `[sp,#k]` accesses — sound for compiler temps, but a
            // volatile local slot could land there once volatile functions optimize; pairing it
            // would merge/reorder volatile accesses (C99 6.7.3). Skip for volatile functions.
            body = pair_ldst(&body); // …then the exposed adjacent accesses → ldp/stp
        }
        let c_pair = costsq.then(|| count_insn_lines(&body));
        if g.regalloc && passes.peephole {
            // collapse forwarder blocks left empty once their φ-copies coalesced away.
            body = thread_asm_branches(&body);
        }
        if costsq {
            let c_final = count_insn_lines(&body);
            cost_log.push((
                f.name.clone(),
                [c_lower.unwrap(), c_load.unwrap(), c_move.unwrap(), c_pair.unwrap(), c_final],
            ));
        }
        g.s.push_str(&body);
        // Phase 1.2: the prologue's `sub sp,sp,#fframe` (emitted before body_start) and the IR
        // body's leading `sub sp,sp,#ir_tspill` are two frame-sizing phases that straddle the
        // body boundary; now that the full function text is rejoined they are adjacent and fuse
        // to one sub (pure sp-arith identity — see fuse_sp_adjust). Scoped to THIS function's
        // region so strict adjacency can never span a function boundary.
        let fused = fuse_sp_adjust(&g.s.split_off(fn_start));
        g.s.push_str(&fused);
        g.s += "\t.cfi_endproc\n";
        _ = writeln!(g.s, "\t.size {0}, .-{0}", f.name);
    }
    emit_module_tail(&mut g, ast);
    if costsq {
        report_cost_square(&cost_log);
    }
    g.s
}


#[cfg(test)]
mod tests;
