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
const GP_BUDGET: ClassBudget = ClassBudget { k: 10, ncaller: 0 }; // NARROW: x19–x28
const GP_BUDGET_WIDE: ClassBudget = ClassBudget { k: 16, ncaller: 6 }; // WIDE: x10–x15 | x19–x28
const FP_BUDGET: ClassBudget = ClassBudget { k: 24, ncaller: 16 };
fn fp_phys(idx: u32) -> u32 {
    if idx < FP_BUDGET.ncaller { 16 + idx } else { 8 + (idx - FP_BUDGET.ncaller) }
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
    // §3: this function uses the WIDE GP budget (x10–x15 caller-saved homes). Set per function
    // in emit_ir_body from the heavy-instruction scan; drives gpp()/gp_ncaller().
    gp_wide: bool,
    csave_gp: Vec<u32>,
    csave_fp: Vec<u32>,
    // Tier-1 #2 (addressing-mode fold): function-wide READ count per temp, computed once
    // per function via the authoritative opt::each_use visitor. A temp used exactly once is
    // safe to fold-and-delete (its sole use is the Load whose address it computes).
    use_count: Vec<u32>,
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
    for &(off, t) in &f.params {
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
    // reg = x29 - off (off may exceed imm12)
    fn lea_local(&mut self, reg: &str, off: u32) {
        if off <= 4095 {
            _ = writeln!(self.s, "\tsub {reg}, x29, #{off}");
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
        match self.a.tt.tys[t as usize] {
            Ty::Float => self.s += "\tldr s0, [x0]\n\tfcvt d0, s0\n\tfmov x0, d0\n",
            // long double: memory binary128 → narrowed to canonical f64 (libgcc rounds correctly)
            Ty::LDouble => self.s += "\tldr q0, [x0]\n\tbl __trunctfdf2\n\tfmov x0, d0\n",
            Ty::Bitfield(b, boff, w) => {
                // load the whole containing unit (unsigned), then shift left/right to isolate the field
                self.s += match self.a.tt.size(b) {
                    1 => "\tldrb w0, [x0]\n",
                    2 => "\tldrh w0, [x0]\n",
                    4 => "\tldr w0, [x0]\n",
                    _ => "\tldr x0, [x0]\n",
                };
                _ = writeln!(self.s, "\tlsl x0, x0, #{}", 64 - boff - w);
                let sh = if self.a.tt.is_unsigned(b) {
                    "lsr"
                } else {
                    "asr"
                };
                _ = writeln!(self.s, "\t{sh} x0, x0, #{}", 64 - w);
            }
            _ => {
                let u = self.a.tt.is_unsigned(t);
                self.s += match (self.a.tt.size(t), u) {
                    (1, false) => "\tldrsb x0, [x0]\n",
                    (1, true) => "\tldrb w0, [x0]\n",
                    (2, false) => "\tldrsh x0, [x0]\n",
                    (2, true) => "\tldrh w0, [x0]\n",
                    (4, false) => "\tldrsw x0, [x0]\n",
                    (4, true) => "\tldr w0, [x0]\n",
                    _ => "\tldr x0, [x0]\n",
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
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb w{rv}, [sp, #{pos}]"),
            2 => writeln!(self.s, "\tstrh w{rv}, [sp, #{pos}]"),
            4 => writeln!(self.s, "\tstr w{rv}, [sp, #{pos}]"),
            _ => writeln!(self.s, "\tstr x{rv}, [sp, #{pos}]"),
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
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tsturb w{rv}, [x29, #-{off}]"),
            2 => writeln!(self.s, "\tsturh w{rv}, [x29, #-{off}]"),
            4 => writeln!(self.s, "\tstur w{rv}, [x29, #-{off}]"),
            _ => writeln!(self.s, "\tstur x{rv}, [x29, #-{off}]"),
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
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb w{rv}, [x{rbase}, #{off}]"),
            2 => writeln!(self.s, "\tstrh w{rv}, [x{rbase}, #{off}]"),
            4 => writeln!(self.s, "\tstr w{rv}, [x{rbase}, #{off}]"),
            _ => writeln!(self.s, "\tstr x{rv}, [x{rbase}, #{off}]"),
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
        if self.a.tt.size(*ct) != 8 {
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
        let rd = self.gp_home(*d).unwrap_or(0);
        if let Some((rbase, off)) = imm_form {
            self.load_gp_off(rd, rbase, off, *lty);
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
            self.tmp_store(*d, "x0");
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
        if self.a.tt.size(*ct) != 8 {
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
        let (base, n) = match (a, b) {
            (Val::Tmp(base), Val::Imm(n)) | (Val::Imm(n), Val::Tmp(base)) => (*base, *n),
            _ => return None,
        };
        let (Some(rbase), Ok(off)) = (self.gp_home(base), u32::try_from(n)) else {
            return None;
        };
        if !self.scaled_off(off, self.a.tt.size(*sty)) {
            return None;
        }
        // read the value from its home (store_gp_off never clobbers it); rbase read as base.
        let rv = self.src_gp(*v, 0);
        self.store_gp_off(rv, rbase, off, *sty);
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
        let rx = self.src_gp(*mx, 0);
        let ry = self.src_gp(*my, 1);
        let ra = self.src_gp(addend, 2);
        let rd = self.gp_home(*d).unwrap_or(0);
        _ = writeln!(self.s, "\tmadd x{rd}, x{rx}, x{ry}, x{ra}");
        self.ext_r(rd, *ctd);
        if self.gp_home(*d).is_none() {
            self.tmp_store(*d, "x0");
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
                let rd = self.gp_home(*d).unwrap_or(0);
                match pos {
                    Some(p) => self.load_gp_sp(rd, p, *lty),
                    None => self.load_gp_fp(rd, *off, *lty),
                }
                if self.gp_home(*d).is_none() {
                    self.tmp_store(*d, "x0");
                }
                Some(2)
            }
            Inst::Store(sty, Val::Tmp(la), v) if la == t && self.simple_gp_store_ty(*sty) => {
                let pos = self.sp_slot_sz(*off, self.a.tt.size(*sty));
                if pos.is_none() && *off > 256 {
                    return None;
                }
                let rv = self.src_gp(*v, 0);
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
    fn store(&mut self, reg: u32, t: TypeId) {
        match self.a.tt.tys[t as usize] {
            Ty::Bool => {
                _ = writeln!(
                    self.s,
                    "\tcmp x{reg}, #0\n\tcset w9, ne\n\tstrb w9, [x1]"
                );
            }
            Ty::Float => {
                _ = writeln!(self.s, "\tfmov d7, x{reg}\n\tfcvt s7, d7\n\tstr s7, [x1]");
            }
            Ty::LDouble => {
                // bl clobbers x1 (caller-saved) — shield the address via the stack
                _ = writeln!(
                    self.s,
                    "\tstr x1, [sp, #-16]!\n\tfmov d0, x{reg}\n\tbl __extenddftf2\n\tldr x1, [sp], #16\n\tstr q0, [x1]"
                );
            }
            Ty::Bitfield(b, boff, w) => {
                // read-modify-write the containing unit
                let usz = self.a.tt.size(b);
                self.s += match usz {
                    1 => "\tldrb w3, [x1]\n",
                    2 => "\tldrh w3, [x1]\n",
                    4 => "\tldr w3, [x1]\n",
                    _ => "\tldr x3, [x1]\n",
                };
                let mask = ((!0u64 >> (64 - w)) << boff) as i64;
                self.imm("x4", mask);
                self.s += "\tbic x3, x3, x4\n";
                _ = writeln!(self.s, "\tlsl x5, x{reg}, #{boff}");
                self.s += "\tand x5, x5, x4\n\torr x3, x3, x5\n";
                self.s += match usz {
                    1 => "\tstrb w3, [x1]\n",
                    2 => "\tstrh w3, [x1]\n",
                    4 => "\tstr w3, [x1]\n",
                    _ => "\tstr x3, [x1]\n",
                };
            }
            _ => {
                _ = match self.a.tt.size(t) {
                    1 => writeln!(self.s, "\tstrb w{reg}, [x1]"),
                    2 => writeln!(self.s, "\tstrh w{reg}, [x1]"),
                    4 => writeln!(self.s, "\tstr w{reg}, [x1]"),
                    _ => writeln!(self.s, "\tstr x{reg}, [x1]"),
                };
            }
        }
    }
    // Copy `sz` bytes: src (x0) → dst (x1), forward. Leaves the dst address in x0 (the
    // rvalue of a struct assignment = the destination address). Shared: AST-path + IR Inst::Memcpy.
    fn blk_copy(&mut self, sz: u32) {
        self.s += "\tmov x4, x1\n";
        if sz > 0 {
            let n = self.labels(1);
            self.imm("x2", sz as i64);
            _ = writeln!(self.s, "L{n}:");
            self.s += "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
            _ = writeln!(self.s, "\tb.ne L{n}");
        }
        self.s += "\tmov x0, x4\n"; // value = dst address
    }
    // Convert the canonical value in x0: from → to
    fn cast_op(&mut self, from: TypeId, to: TypeId) {
        let tt = &self.a.tt;
        if matches!(
            tt.tys[to as usize],
            Ty::Void | Ty::Struct(_) | Ty::Array(..)
        ) {
            return;
        }
        match (tt.is_float(from), tt.is_float(to)) {
            (false, false) => self.ext(to),
            (false, true) => {
                let cvt = if tt.is_unsigned(from) {
                    "ucvtf"
                } else {
                    "scvtf"
                };
                _ = writeln!(self.s, "\t{cvt} d0, x0");
                if tt.size(to) == 4 {
                    self.s += "\tfcvt s0, d0\n\tfcvt d0, s0\n";
                }
                self.s += "\tfmov x0, d0\n";
            }
            (true, false) => {
                if matches!(tt.tys[to as usize], Ty::Bool) {
                    self.s += "\tfmov d0, x0\n\tfcmp d0, #0.0\n\tcset x0, ne\n";
                    return;
                }
                self.s += "\tfmov d0, x0\n";
                let cvt = if self.a.tt.is_unsigned(to) {
                    "fcvtzu"
                } else {
                    "fcvtzs"
                };
                if self.a.tt.size(to) == 8 {
                    _ = writeln!(self.s, "\t{cvt} x0, d0");
                } else {
                    _ = writeln!(self.s, "\t{cvt} w0, d0");
                    self.ext(to);
                }
            }
            (true, true) => {
                if tt.size(to) == 4 {
                    self.s += "\tfmov d0, x0\n\tfcvt s0, d0\n\tfcvt d0, s0\n\tfmov x0, d0\n";
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
        let sy = sym(name);
        if self.a.funcs.iter().any(|f| f.name == name && f.is_static) {
            _ = writeln!(self.s, "\tadrp x0, {0}\n\tadd x0, x0, :lo12:{0}", sy);
        } else {
            _ = writeln!(self.s, "\tadrp x0, :got:{0}\n\tldr x0, [x0, :got_lo12:{0}]", sy);
        }
    }
    // memset(x0, 0, sz): zero sz bytes starting at the address in x0. Shared by the
    // AST-walk (Node::Zero) and IR (Inst::Zero). sz==0 → no-op.
    fn emit_zero(&mut self, sz: u32) {
        if sz == 0 {
            return;
        }
        self.imm("x2", sz as i64);
        let n = self.labels(1);
        _ = writeln!(self.s, "L{n}:");
        self.s += "\tstrb wzr, [x0], #1\n\tsubs x2, x2, #1\n";
        _ = writeln!(self.s, "\tb.ne L{n}");
    }
    // EXT(gcc): &&label (computed-goto) → x0. A local label within the current function.
    fn emit_labeladdr(&mut self, name: &str) {
        _ = writeln!(
            self.s,
            "\tadrp x0, lg_{0}.{1}\n\tadd x0, x0, :lo12:lg_{0}.{1}",
            self.fname, name
        );
    }
    // __builtin_*_overflow: x0=a, x1=b, x9=&res. bool result → x0. Shared by the
    // AST-walk (Node::Overflow) + IR (Inst::Overflow). ta/tb/rt = types of a/b/*rp.
    fn emit_overflow(&mut self, op: u8, ta: TypeId, tb: TypeId, rt: TypeId) {
        let a_sg = !self.a.tt.is_unsigned(ta);
        let b_sg = !self.a.tt.is_unsigned(tb);
        let (r_sg, rw) = (!self.a.tt.is_unsigned(rt), self.a.tt.size(rt));
        crate::ext::overflow_emit(&mut self.s, op, a_sg, b_sg, r_sg, rw);
    }
    // va_start: x0 = &ap. Fill the AAPCS va_list from the prologue state (va=gp,fp,stk,frame).
    // Shared by the AST-walk (Node::VaStart) + IR (Inst::VaStart).
    fn emit_vastart(&mut self) {
        let (gp, fp, stk, frame) = self.va;
        self.imm("x9", (16 + stk) as i64);
        self.s += "\tadd x9, x29, x9\n\tstr x9, [x0]\n"; // __stack
        self.imm("x9", frame as i64);
        self.s += "\tsub x9, x29, x9\n\tstr x9, [x0, #8]\n"; // __gr_top
        self.s += "\tsub x9, x9, #64\n\tstr x9, [x0, #16]\n"; // __vr_top
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x0, #24]", (gp as i64 - 8) * 8);
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x0, #28]", (fp as i64 - 8) * 16);
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
            if idx < GP_BUDGET_WIDE.ncaller { 10 + idx } else { 19 + (idx - GP_BUDGET_WIDE.ncaller) }
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
        if self.a.tt.is_float(ct) {
            self.s += "\tfmov d0, x0\n\tfmov d1, x1\n";
            match op {
                Op::Add => self.s += "\tfadd d0, d0, d1\n\tfmov x0, d0\n",
                Op::Sub => self.s += "\tfsub d0, d0, d1\n\tfmov x0, d0\n",
                Op::Mul => self.s += "\tfmul d0, d0, d1\n\tfmov x0, d0\n",
                Op::Div => self.s += "\tfdiv d0, d0, d1\n\tfmov x0, d0\n",
                _ => {
                    let cond = match op {
                        Op::Eq => "eq", Op::Ne => "ne", Op::Lt => "mi",
                        Op::Le => "ls", Op::Gt => "gt", Op::Ge => "ge",
                        _ => unreachable!(),
                    };
                    _ = writeln!(self.s, "\tfcmp d0, d1\n\tcset x0, {cond}");
                }
            }
            return;
        }
        let u = self.a.tt.is_unsigned(ct);
        match op {
            Op::Add => self.s += "\tadd x0, x0, x1\n",
            Op::Sub => self.s += "\tsub x0, x0, x1\n",
            Op::Mul => self.s += "\tmul x0, x0, x1\n",
            Op::Div if u => self.s += "\tudiv x0, x0, x1\n",
            Op::Div => self.s += "\tsdiv x0, x0, x1\n",
            Op::Rem if u => self.s += "\tudiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
            Op::Rem => self.s += "\tsdiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
            Op::And => self.s += "\tand x0, x0, x1\n",
            Op::Or => self.s += "\torr x0, x0, x1\n",
            Op::Xor => self.s += "\teor x0, x0, x1\n",
            Op::Shl => self.s += "\tlsl x0, x0, x1\n",
            Op::Shr if u => self.s += "\tlsr x0, x0, x1\n",
            Op::Shr => self.s += "\tasr x0, x0, x1\n",
            _ => {
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                _ = writeln!(self.s, "\tcmp x0, x1\n\tcset x0, {cond}");
                return; // 0/1, no ext needed
            }
        }
        if self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4 {
            self.ext(ct);
        }
    }

    // Tier-1 #1 — compute-into-home: x{rd} = x{ra} ⟨op⟩ x{rb}, canonical per ct, with NO
    // x0/x1 funnel. A register-parametric transcription of the integer arm of ir_bin: for
    // rd=ra=0,rb=1 it emits BYTE-IDENTICAL asm to `ir_bin` (the x0-funnel), so the -O0 path
    // (all temps spilled ⟹ rd=ra=0,rb=1) is unchanged; only the register-resident path skips
    // the copies. Correctness = ir_bin's (same mnemonic per Op); validated by opt-parity.
    // x2 is a fixed scratch for rem's quotient (never a home: the home set is x10–x15 ∪ x19–x28,
    // WIDE or NARROW — x2 lies outside both budgets); msub reads
    // all sources before writing rd, so rd may alias ra/rb (the allocator only coalesces when
    // the aliased source is dead here). No ext on the compare path (cset yields a clean 0/1).
    fn ir_bin_r(&mut self, op: Op, ct: TypeId, rd: u32, ra: u32, rb: u32) {
        let u = self.a.tt.is_unsigned(ct);
        match op {
            Op::Add => _ = writeln!(self.s, "\tadd x{rd}, x{ra}, x{rb}"),
            Op::Sub => _ = writeln!(self.s, "\tsub x{rd}, x{ra}, x{rb}"),
            Op::Mul => _ = writeln!(self.s, "\tmul x{rd}, x{ra}, x{rb}"),
            Op::Div if u => _ = writeln!(self.s, "\tudiv x{rd}, x{ra}, x{rb}"),
            Op::Div => _ = writeln!(self.s, "\tsdiv x{rd}, x{ra}, x{rb}"),
            Op::Rem if u => {
                _ = writeln!(self.s, "\tudiv x2, x{ra}, x{rb}\n\tmsub x{rd}, x2, x{rb}, x{ra}")
            }
            Op::Rem => {
                _ = writeln!(self.s, "\tsdiv x2, x{ra}, x{rb}\n\tmsub x{rd}, x2, x{rb}, x{ra}")
            }
            Op::And => _ = writeln!(self.s, "\tand x{rd}, x{ra}, x{rb}"),
            Op::Or => _ = writeln!(self.s, "\torr x{rd}, x{ra}, x{rb}"),
            Op::Xor => _ = writeln!(self.s, "\teor x{rd}, x{ra}, x{rb}"),
            Op::Shl => _ = writeln!(self.s, "\tlsl x{rd}, x{ra}, x{rb}"),
            Op::Shr if u => _ = writeln!(self.s, "\tlsr x{rd}, x{ra}, x{rb}"),
            Op::Shr => _ = writeln!(self.s, "\tasr x{rd}, x{ra}, x{rb}"),
            _ => {
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                _ = writeln!(self.s, "\tcmp x{ra}, x{rb}\n\tcset x{rd}, {cond}");
                return; // 0/1, no ext needed
            }
        }
        if self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4 {
            self.ext_r(rd, ct);
        }
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
                _ = writeln!(self.s, "\t{mnem} x{ra}, #{mag}\n\tcset x{rd}, {cond}");
                true // 0/1 result, no ext
            }
            Op::Shl | Op::Shr => {
                if !(0..64).contains(&k) {
                    return false;
                }
                let mnem = match op {
                    Op::Shl => "lsl",
                    Op::Shr if u => "lsr",
                    _ => "asr",
                };
                _ = writeln!(self.s, "\t{mnem} x{rd}, x{ra}, #{k}");
                if is4 {
                    self.ext_r(rd, ct);
                }
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
                // loaded exactly this pattern via imm(), so the fold is byte-equivalent.
                _ = writeln!(self.s, "\t{mnem} x{rd}, x{ra}, #{}", k as u64);
                if is4 {
                    self.ext_r(rd, ct);
                }
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

    fn ir_call(&mut self, dst: &Option<Tmp>, callee: &Callee, args: &[Val]) {
        // INVARIANT (ir::call_composite, ir.rs): Inst::Call carries ONLY ≤8 GP + ≤8 FP
        // SCALAR args — any register overflow (gp>8 / fp>8), composite (struct/HFA/>16B),
        // or non-8B float is routed to Inst::CallX/ir_call_abi instead. So neither counter
        // can reach 8 here; the debug_asserts pin that delegated precondition.
        let (mut gp, mut fp) = (0u32, 0u32);
        for &a in args {
            if self.val_is_float(a) {
                debug_assert!(fp < 8, "Inst::Call fp>8 — call_composite must route to CallX");
                self.ld_val(a, "x9");
                _ = writeln!(self.s, "\tfmov d{fp}, x9");
                fp += 1;
            } else {
                debug_assert!(gp < 8, "Inst::Call gp>8 — call_composite must route to CallX");
                let r = format!("x{gp}");
                self.ld_val(a, &r);
                gp += 1;
            }
        }
        match callee {
            Callee::Sym(name) => _ = writeln!(self.s, "\tbl {}", sym(name)),
            Callee::Ptr(p) => {
                self.ld_val(*p, "x9");
                self.s += "\tblr x9\n";
            }
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
                    self.ld_val(val, "x0");
                    if fl && sz == 16 {
                        _ = writeln!(self.s, "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #{o}]");
                    } else if fl && sz == 4 {
                        _ = writeln!(self.s, "\tfmov d7, x0\n\tfcvt s7, d7\n\tstr s7, [sp, #{o}]");
                    } else {
                        _ = match sz {
                            1 => writeln!(self.s, "\tstrb w0, [sp, #{o}]"),
                            2 => writeln!(self.s, "\tstrh w0, [sp, #{o}]"),
                            4 => writeln!(self.s, "\tstr w0, [sp, #{o}]"),
                            _ => writeln!(self.s, "\tstr x0, [sp, #{o}]"),
                        };
                    }
                }
                ASlot::StS(o, sz) => {
                    self.ld_val(val, "x0"); // x0 = struct address
                    let mut k = 0;
                    while k < sz {
                        _ = writeln!(self.s, "\tldr x8, [x0, #{k}]\n\tstr x8, [sp, #{}]", o + k);
                        k += 8;
                    }
                }
                _ => {}
            }
        }
        if let Callee::Ptr(p) = callee {
            self.ld_val(*p, "x0");
            self.s += "\tstr x0, [sp, #-16]!\n";
        }
        let regargs: Vec<(Val, ASlot)> = args
            .iter()
            .zip(&plan)
            .filter(|(_, sl)| !matches!(sl, ASlot::S(..) | ASlot::StS(..)))
            .map(|(&(v, _), &sl)| (v, sl))
            .collect();
        for &(val, sl) in &regargs {
            self.ld_val(val, "x0"); // struct: x0 = address
            if matches!(sl, ASlot::Q(_)) {
                self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #-16]!\n";
            } else {
                self.s += "\tstr x0, [sp, #-16]!\n";
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
                    let ra = self.src_gp(*a, 0);
                    if ra != 0 {
                        _ = writeln!(self.s, "\tmov x0, x{ra}");
                    }
                    self.tmp_store(*d, "x0");
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
                let rc = self.src_gp(*c, 0);
                let ra = self.src_gp(*a, 1);
                let rb = self.src_gp(*b, 2);
                let rd = self.gp_home(*d).unwrap_or(0);
                _ = writeln!(self.s, "\tcmp x{rc}, #0\n\tcsel x{rd}, x{ra}, x{rb}, ne");
                self.ext_r(rd, *ty);
                if self.gp_home(*d).is_none() {
                    self.tmp_store(*d, "x0");
                }
            }
            Inst::Bin(d, op, ty, a, b) => {
                if self.a.tt.is_float(*ty) {
                    self.ld_val(*a, "x0");
                    self.ld_val(*b, "x1");
                    self.ir_bin(*op, *ty);
                    self.tmp_store(*d, "x0");
                } else if let Some((mnem, mag)) = add_sub_imm12(*op, *b) {
                    // Add/Sub-immediate peephole (Side-II: AAPCS64 imm12 field, unsigned
                    // 0..4096): fold a small constant operand into the instruction instead of
                    // materializing it (`mov x1,#k; add` → `add xD,xA,#k`). Pressure-free —
                    // one fewer scratch live per loop increment; validated by opt-parity.
                    let ra = self.src_gp(*a, 0);
                    let rd = self.gp_home(*d).unwrap_or(0);
                    _ = writeln!(self.s, "\t{mnem} x{rd}, x{ra}, #{mag}");
                    if self.a.tt.is_integer(*ty) && self.a.tt.size(*ty) == 4 {
                        self.ext_r(rd, *ty);
                    }
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, "x0");
                    }
                } else {
                    // Tier-1 #1: read operands from their homes, compute into d's home.
                    // Sources first (x0/x1 scratch for spilled/imm), THEN pick rd — if d is
                    // spilled, rd=0 (x0) and the result is stored after; a/b are already
                    // consumed into their own scratch/home so x0-as-rd cannot clobber them.
                    let ra = self.src_gp(*a, 0);
                    let rd = self.gp_home(*d).unwrap_or(0);
                    // Immediate-operand instruction-selection fold (§4): a compare/shift/logical
                    // with a constant right operand folds the constant into the instruction's
                    // imm field instead of materializing it into a scratch (`mov x1,#k; op` →
                    // `op …,#k`) — byte-equivalent since imm() would load exactly #k. Kills the
                    // ~14k `mov xR,#N` that feed cmp/shift/bitmask. opt-parity certifies.
                    if !self.try_bin_imm(*op, *ty, rd, ra, *b) {
                        let rb = self.src_gp(*b, 1);
                        self.ir_bin_r(*op, *ty, rd, ra, rb);
                    }
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, "x0");
                    }
                }
            }
            Inst::Un(d, u, ty, a) => {
                // Float neg keeps the x0 funnel (fmov round-trip). Integer neg/not is the
                // Tier-1 #1 compute-into-home path: read a from its home, write d's home,
                // ext in place. rd=ra=0 ⟹ byte-identical to the old `neg x0,x0; ext` path.
                if matches!(u, Un::Neg) && self.a.tt.is_float(*ty) {
                    self.ld_val(*a, "x0");
                    self.s += "\tfmov d0, x0\n\tfneg d0, d0\n\tfmov x0, d0\n";
                    self.tmp_store(*d, "x0");
                } else {
                    let ra = self.src_gp(*a, 0);
                    let rd = self.gp_home(*d).unwrap_or(0);
                    match u {
                        Un::Neg => _ = writeln!(self.s, "\tneg x{rd}, x{ra}"),
                        Un::BNot => _ = writeln!(self.s, "\tmvn x{rd}, x{ra}"),
                    }
                    self.ext_r(rd, *ty);
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, "x0");
                    }
                }
            }
            Inst::Load(d, ty, a) => {
                if self.simple_gp_load_ty(*ty) {
                    // Tier-1 #2 groundwork: address from its home, load into d's home.
                    let ra = self.src_gp(*a, 0);
                    let rd = self.gp_home(*d).unwrap_or(0);
                    self.load_gp(rd, ra, *ty);
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, "x0");
                    }
                } else {
                    self.ld_val(*a, "x0");
                    self.load(*ty);
                    self.tmp_store(*d, "x0");
                }
            }
            Inst::Store(ty, a, v) => {
                // Read the value from its home (no `mov x0,vHome` funnel) — store() is now
                // clobber-free so passing a live home is safe. Address still funnels to x1
                // (store hardcodes [x1]); loading it there cannot clobber a spilled v in x0.
                let rv = self.src_gp(*v, 0);
                self.ld_val(*a, "x1");
                self.store(rv, *ty);
            }
            Inst::Memcpy(d, s, sz) => {
                self.ld_val(*s, "x0"); // src
                self.ld_val(*d, "x1"); // dst
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
                        let reg = rd.map(|r| format!("x{r}")).unwrap_or_else(|| "x0".into());
                        self.lea_local(&reg, *off);
                        if rd.is_none() {
                            self.tmp_store(*d, "x0");
                        }
                    }
                    Place::Global(name, off) => {
                        let rd = self.gp_home(*d);
                        self.lea_global(rd.unwrap_or(0), name, *off);
                        if rd.is_none() {
                            self.tmp_store(*d, "x0");
                        }
                    }
                    Place::Str(i) => {
                        let rd = self.gp_home(*d);
                        let reg = rd.unwrap_or(0);
                        _ = writeln!(
                            self.s,
                            "\tadrp x{reg}, l_str{0}\n\tadd x{reg}, x{reg}, :lo12:l_str{0}",
                            i
                        );
                        if rd.is_none() {
                            self.tmp_store(*d, "x0");
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
                    let ra = self.src_gp(*a, 0);
                    let rd = self.gp_home(*d).unwrap_or(0);
                    self.ext_rd(rd, ra, *to);
                    if self.gp_home(*d).is_none() {
                        self.tmp_store(*d, "x0");
                    }
                } else {
                    self.ld_val(*a, "x0");
                    self.cast_op(*from, *to);
                    self.tmp_store(*d, "x0");
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
                self.tmp_store(*d, "x0");
            }
            Inst::LabelAddr(d, name) => {
                self.emit_labeladdr(name);
                self.tmp_store(*d, "x0");
            }
            Inst::Zero(a, sz) => {
                self.ld_val(*a, "x0"); // address → x0
                self.emit_zero(*sz);
            }
            Inst::VaStart(a) => {
                self.ld_val(*a, "x0"); // &ap → x0
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
                _ = writeln!(self.s, "\tadd x0, x29, #{off}");
                self.tmp_store(*d, "x0");
            }
            Inst::GotoPtr(a) => {
                self.ld_val(*a, "x0");
                self.s += "\tbr x0\n";
            }
            Inst::Alloca(d, size) => {
                self.ld_val(*size, "x0"); // byte count
                self.s += "\tadd x0, x0, #15\n\tand x0, x0, #0xfffffffffffffff0\n\tsub sp, sp, x0\n\tmov x0, sp\n";
                self.tmp_store(*d, "x0");
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
        let ra = self.src_gp(a, 0);
        // b: fold small ±imm12 into cmp/cmn (byte-identical to try_bin_imm), else a register.
        if let Val::Imm(k) = b
            && (0..4096).contains(&k)
        {
            _ = writeln!(self.s, "\tcmp x{ra}, #{k}");
        } else if let Val::Imm(k) = b
            && (-4095..=0).contains(&k)
        {
            _ = writeln!(self.s, "\tcmn x{ra}, #{}", -k);
        } else {
            let rb = self.src_gp(b, 1);
            _ = writeln!(self.s, "\tcmp x{ra}, x{rb}");
        }
        let (lt, le) = (self.ir_label(tb), self.ir_label(eb));
        let ncc = inv_cond(cc);
        if ft == Some(eb) {
            _ = writeln!(self.s, "\tb.{cc} {lt}"); // else falls through: take THEN on cc
        } else if ft == Some(tb) {
            _ = writeln!(self.s, "\tb.{ncc} {le}"); // then falls through: take ELSE on ¬cc
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
                let rc = self.src_gp(*c, 0);
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
            matches!(i, Inst::Overflow(..) | Inst::Sync(..) | Inst::VaArg(..))
                || matches!(i, Inst::Load(_, ty, _) | Inst::Store(ty, _, _)
                    if matches!(tt.tys[*ty as usize], Ty::LDouble))
        });
        self.gp_wide = self.regalloc && !heavy;
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
        // near_branch: an upper bound on emitted size (≤12 insns per IR inst + 4 per block).
        // Under 200k emitted insns (~800 KB) every label is well within cb(n)z's ±1MB reach.
        let est = irf.blocks.iter().map(|b| b.insts.len()).sum::<usize>() * 12 + irf.blocks.len() * 4;
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
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BACKEND PEEPHOLE (Phase C) — machine-level redundant register-move elimination.
//
// WHY (MEASURED, not assumed): the emitter is an x0-accumulator machine ("every scalar
// lives in x0", top-of-file). The Stage-5b allocator gives each IR temp a HOME register,
// but the emitter still routes every op through x0/x1 and copies to/from the home — so a
// value is stored to its home (`mov xH, x0`) and immediately reloaded (`mov x0, xH`). On
// matmul this makes 197 of 398 instructions reg-reg `mov`s (gcc-O0: 0). This pass removes
// the provably-redundant ones — the single biggest measured lever toward QBE-class codegen.
//
// SEMANTICS PRESERVED — the safety argument (machine-level translation validation):
//   Track, within a STRAIGHT-LINE region, a value-equivalence over 64-bit GP registers: a
//   `mov xD, xS` makes D≡S (they hold the identical 64-bit value). The ONLY rewrite is:
//   DROP a `mov xD, xS` when the model already proves D≡S — the copy is then a verified
//   no-op, so removing it cannot change any later observation. The model stays SOUND because
//   every value-changing event breaks the relevant equivalence:
//     • a recognized DEF (first-operand-writing instruction) gives its destination a FRESH
//       value id — so no stale equivalence to it survives;
//     • an unrecognized mnemonic, any branch/call/label (a basic-block boundary) FLUSHES the
//       whole model — we never reason across control flow or an instruction we don't model.
//   32-bit (`w`) writes and float ops that define a GP reg still invalidate that register's
//   slot; equivalences are FORMED only by full-width `mov x,x`, so a partial-width write can
//   never be mistaken for a 64-bit copy. Live-out is safe: a redundant `mov x0, xH` at a
//   region end is dropped only when x0 ALREADY holds xH's value, so the return/epilogue sees
//   the same x0. Correctness is re-validated end-to-end by opt-parity (0 DIVERGE) + torture.
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `mov xD, xS` (both 64-bit GP) → (D, S); None for `mov x,#imm` / `mov w,w` / shifts.
fn parse_mov_xx(t: &str) -> Option<(u32, u32)> {
    let rest = t.strip_prefix("mov ")?;
    let mut it = rest.split(',');
    let d = it.next()?.trim().strip_prefix('x')?.parse::<u32>().ok()?;
    let s = it.next()?.trim().strip_prefix('x')?.parse::<u32>().ok()?;
    if it.next().is_some() {
        return None; // a third operand (shift) ⟹ not a plain reg-reg move
    }
    Some((d, s))
}

/// The slot of the first register operand (x or w share a physical slot), for DEF tracking.
fn first_reg_slot(operands: &str) -> Option<u32> {
    let tok = operands.split(',').next()?.trim();
    tok.strip_prefix('x').or_else(|| tok.strip_prefix('w'))?.parse::<u32>().ok()
}

/// The register slots an instruction READS and WRITES, plus whether it ends a straight-line
/// region (branch/call/ret/unknown/writeback-addressing ⟹ we stop reasoning). Only x/w GP
/// registers are tracked; sp/fp/float operands are ignored (they never form a `mov x,x` we
/// rewrite, and over-counting a read only KEEPS more moves — the safe direction).
fn reg_uses(t: &str) -> (Vec<u32>, Vec<u32>, bool) {
    // Writeback / pre-post-index addressing mutates the base register implicitly — rather
    // than model it, treat the line as a region boundary (conservative = keep everything).
    if t.contains('!') || t.contains("],") {
        return (vec![], vec![], true);
    }
    let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    let operands = t[mn.len()..].trim_start();
    // A GP-register slot in one operand TOKEN, or None if the token is a float/vector reg
    // (q/d/s/v/h/b), an immediate, a label, or a condition. Brackets (memory `[x0]`) stripped.
    let slot = |tok: &str| -> Option<u32> {
        let tok = tok.trim().trim_start_matches('[').trim_end_matches(']');
        tok.strip_prefix('x').or_else(|| tok.strip_prefix('w'))?.parse::<u32>().ok()
    };
    // Operand tokens, POSITIONALLY (comma-split). The destination of a def-first instruction
    // is token[0]; a memory operand like `[x0, x1]` splits into two tokens, both address READS.
    let toks: Vec<&str> = operands.split(',').collect();
    let gp_in = |range: &[&str]| -> Vec<u32> { range.iter().filter_map(|tk| slot(tk)).collect() };
    const BOUNDARY: &[&str] =
        &["b", "bl", "blr", "br", "ret", "cbz", "cbnz", "tbz", "tbnz"];
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    const DEF_FIRST: &[&str] = &[
        "mov", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn", "and",
        "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth", "sxtb",
        "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr", "ldrb",
        "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd", "fsub",
        "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl",
    ];
    if mn.starts_with("b.") || BOUNDARY.contains(&mn) {
        (vec![], vec![], true)
    } else if NO_DEF.contains(&mn) {
        (gp_in(&toks), vec![], false) // stores/compares: every GP operand is a READ
    } else if mn == "ldp" {
        // token[0], token[1] are destinations; the rest are address READS.
        let n = toks.len().min(2);
        (gp_in(&toks[n..]), gp_in(&toks[..n]), false)
    } else if mn == "movk" {
        (gp_in(&toks), gp_in(&toks[..toks.len().min(1)]), false) // merge: reads its own dst too
    } else if DEF_FIRST.contains(&mn) {
        // token[0] is the destination POSITION. If it is a GP reg → the WRITE; if it is a
        // float/vector reg (q0/d0/s0/…) → NO GP write, and every GP operand is a READ (the
        // bug this fixes: `ldr q0, [x0]` / `fmov d0, x0` must NOT treat x0 as the destination).
        match toks.split_first() {
            Some((first, rest)) => match slot(first) {
                Some(d) => (gp_in(rest), vec![d], false),
                None => (gp_in(rest), vec![], false),
            },
            None => (vec![], vec![], false),
        }
    } else {
        (vec![], vec![], true) // unknown ⟹ boundary (never mis-model)
    }
}

/// Machine-level move cleanup over one function body (see the block comment).
/// COPY PROPAGATION first (funnel every read to its value's producer, so the x0-scratch
/// copies the emitter inserts become dead), THEN redundant round-trips, THEN dead stores.
fn peephole_moves(body: &str) -> String {
    drop_dead_moves(&drop_redundant_moves(&propagate_copies(body)))
}

// ─────────────────────────────────────────────────────────────────────────────
// MACHINE-LEVEL COPY PROPAGATION (Tier-A, pressure-FREE). [OPT.md §4 diagnostic:
// the matmul inner k-loop carried 10/39 reg-reg `mov`s — the emitter's x0 funnel
// (`mov x0, xSRC; mov xDST, x0`; `<compute> x0; mov xDST, x0`). These are pure copies:
// removing one LOWERS register pressure, so this is a WIN independent of the pressure
// guard (§2). The funnel also inflated measured SSA pressure, which is what blocked LICM
// from hoisting the invariant `adrp` (the §4 anomaly, cause (c)).]
//
// TRANSFORM. Within a straight-line region, maintain `home[r]` = the register that
// canonically holds r's current value (the ROOT of its copy chain), formed ONLY by a
// full-width `mov x,x`. REWRITE every READ operand `r` to `home[r]`. This funnels each read
// back to the value's producer, so the intermediate scratch copies are read by nothing and
// die (removed by `drop_dead_moves`). No line is deleted here — only read registers renamed
// among provably-equal registers.
//
// SOUNDNESS (same model as drop_redundant_moves — machine translation validation):
//   `home[r]=c` is established only when r and c provably hold the identical 64-bit value
//   (a `mov x,x`), so substituting a read r→c cannot change any value. The model stays sound
//   because every value-changing event severs the stale link:
//     • a real DEF of register D (any first-operand write that is NOT a copy) makes D its own
//       root AND severs every x with home[x]==D — those x still hold the OLD value at x, so
//       their root becomes x, never the redefined D;
//     • a `mov xD,xS` first severs D (its value is being replaced), then sets home[D]=root(S);
//     • any label / branch / call / unknown mnemonic / writeback-addressing FLUSHES the model
//       (we never reason across a boundary).
//   A `w` (32-bit) read substitutes to the `w` form of the root: full-64 equality implies
//   low-32 equality, so it is safe. Only x/w GP registers are tracked; sp/fp/vector operands
//   never match the substitution scan. Re-validated end-to-end by opt-parity (0 DIVERGE) +
//   torture — exactly the net that guards the existing peephole.
// ─────────────────────────────────────────────────────────────────────────────

/// Substitute the single GP register in one operand token by its `home` root (letter and
/// surrounding syntax — brackets, offset — preserved). Immediates, symbols, conditions, FP
/// registers, and `sp` never match (no `x`/`w` + digits), so they pass through untouched.
fn sub_reg_token(tok: &str, home: &std::collections::HashMap<u32, u32>) -> String {
    // A relocation / symbol operand (`:lo12:x00`, `:got:`) can hold a C global whose name
    // looks exactly like a register (`x00`) — NEVER a register, so never substitute it. The
    // ':' marks it unambiguously; adrp's bare-symbol operand is skipped at the call site.
    if tok.contains(':') {
        return tok.to_string();
    }
    let b = tok.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // a register starts with x/w NOT preceded by an alphanumeric (so the 'x' inside a
        // symbol like ".Lx3" or "lo12" is never mistaken for a register).
        if (c == b'x' || c == b'w') && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // A real GP register is `x0`..`x30` — 1–2 digits, value ≤ 30, NO leading zero.
            // A symbol like `x00` (leading zero) or `x40` (>30) fails this and is left alone.
            let digits = &tok[i + 1..j];
            let canonical = j > i + 1
                && (j == b.len() || !b[j].is_ascii_alphanumeric())
                && (digits.len() == 1 || !digits.starts_with('0'));
            if canonical {
                if let Ok(n) = digits.parse::<u32>() {
                    if n <= 30 {
                        let r = *home.get(&n).unwrap_or(&n);
                        if r != n {
                            return format!("{}{}{}{}", &tok[..i], c as char, r, &tok[j..]);
                        }
                        return tok.to_string();
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    tok.to_string()
}

/// Copy-propagate read operands to their value's producer within each straight-line region.
fn propagate_copies(body: &str) -> String {
    use std::collections::HashMap;
    let mut out = String::with_capacity(body.len());
    let mut home: HashMap<u32, u32> = HashMap::new(); // reg → canonical root holding its value
    // Sever register D: it is being (re)defined, so nothing may read the OLD value through it.
    let sever = |home: &mut HashMap<u32, u32>, d: u32| {
        home.retain(|_, v| *v != d); // copies of the old D value: root becomes themselves
        home.remove(&d); // D is now its own root (holds the fresh value)
    };
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    const DEF_FIRST: &[&str] = &[
        "mov", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn", "and",
        "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth", "sxtb",
        "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr", "ldrb",
        "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd", "fsub",
        "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl",
    ];
    // Boundary mnemonics that still READ a register before the region ends (fold the read,
    // then flush). Plain b/bl/ret carry no GP read we propagate.
    const READ_THEN_FLUSH: &[&str] = &["cbz", "cbnz", "tbz", "tbnz", "br", "blr"];
    let gp = |tok: &str| -> Option<u32> {
        let t = tok.trim().trim_start_matches('[').trim_end_matches(']');
        t.strip_prefix('x').or_else(|| t.strip_prefix('w'))?.parse::<u32>().ok()
    };
    for line in body.lines() {
        let t = line.trim();
        // Label FIRST — an emitted label `.Lir_x:` both starts with '.' and ends with ':';
        // it is a basic-block boundary and MUST flush before the directive fast-path.
        if t.ends_with(':') {
            home.clear(); // label = basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.is_empty() || t.starts_with('.') {
            out.push_str(line); // blank / directive — no register effect
            out.push('\n');
            continue;
        }
        // Writeback / pre-post-index mutates the base implicitly ⟹ boundary (never model it).
        if t.contains('!') || t.contains("],") {
            home.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        let toks: Vec<&str> = operands.split(',').collect();
        // Rewrite the chosen READ tokens; leave the destination position and update `home`.
        let emit = |toks: &[String]| -> String { format!("\t{} {}", mn, toks.join(",")) };
        let sub_all = |toks: &[&str], home: &HashMap<u32, u32>| -> Vec<String> {
            toks.iter().map(|tk| sub_reg_token(tk, home)).collect()
        };
        if mn.starts_with("b.") || matches!(mn, "b" | "bl" | "ret") {
            out.push_str(line);
            out.push('\n');
            home.clear();
            continue;
        }
        if READ_THEN_FLUSH.contains(&mn) {
            let nt = sub_all(&toks, &home); // the register operands are reads
            out.push_str(&emit(&nt));
            out.push('\n');
            home.clear();
            continue;
        }
        if NO_DEF.contains(&mn) {
            let nt = sub_all(&toks, &home); // stores/compares: every operand is a read
            out.push_str(&emit(&nt));
            out.push('\n');
            continue;
        }
        if mn == "ldp" {
            // token[0], token[1] are destinations (leave); the rest are address reads.
            let n = toks.len().min(2);
            let mut nt: Vec<String> = toks[..n].iter().map(|s| s.to_string()).collect();
            nt.extend(sub_all(&toks[n..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            for tk in &toks[..n] {
                if let Some(d) = gp(tk) {
                    sever(&mut home, d);
                }
            }
            continue;
        }
        if mn == "movk" {
            // token[0] is a read+write merge (leave it — substituting the accumulator dst is
            // wrong); the rest are reads. The partial write severs the dst.
            let mut nt = vec![toks[0].to_string()];
            nt.extend(sub_all(&toks[1..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            if let Some(d) = gp(toks[0]) {
                sever(&mut home, d);
            }
            continue;
        }
        if mn == "adrp" {
            // `adrp xD, SYM` — the second operand is a BARE symbol (a global named `x5` would
            // masquerade as a register); never substitute it. Just record the fresh dst.
            out.push_str(line);
            out.push('\n');
            if let Some(d) = toks.first().and_then(|s| gp(s)) {
                sever(&mut home, d);
            }
            continue;
        }
        if DEF_FIRST.contains(&mn) {
            let dst = toks.first().and_then(|s| gp(s));
            let mut nt = vec![toks[0].to_string()]; // destination stays
            nt.extend(sub_all(&toks[1..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            // Update the model. A FULL-WIDTH `mov x,x` is a 64-bit COPY: D takes S's root.
            // Any other GP write — including a narrow `mov w,w`, which zero-extends the low
            // 32 bits and so produces a DIFFERENT 64-bit value (the bswap-1 truncation bug) —
            // gives D a fresh value and records NO equivalence. `parse_mov_xx` accepts only
            // `mov x,x`, exactly the copies drop_redundant_moves already trusts. A float/vector
            // dst (gp() == None) touches no GP reg.
            if let Some(d) = dst {
                // Resolve S's root BEFORE severing D — sever may drop entries that point at D
                // (e.g. `mov x0,x24` when x24 was itself a copy of x0), and we must read the
                // root as it stood before this instruction. rs==d ⟹ the copy is redundant
                // (D already holds its own value) ⟹ record nothing, keeping D its own root.
                let rs = parse_mov_xx(t).map(|(_, s)| *home.get(&s).unwrap_or(&s));
                sever(&mut home, d);
                if let Some(rs) = rs {
                    if rs != d {
                        home.insert(d, rs);
                    }
                }
            }
            continue;
        }
        // Unknown mnemonic ⟹ boundary (never mis-model).
        out.push_str(line);
        out.push('\n');
        home.clear();
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// B4 — LOAD/STORE PAIR FORMATION (`ldp`/`stp`). [Side-I structural theorem —
// OPT.md §5 (B4) / §6 Tier-5 #23.]
//
// THEOREM. Two ADJACENT same-class accesses to `[base,#o]` and `[base,#o+sz]` (sz the
// access width) have the SAME memory effect as one pair op `ldp/stp rA,rB,[base,#o]`
// — the paired form transfers exactly the two words at the two addresses. Merging two
// consecutive lines introduces no reordering (nothing executes between them) and the
// disjoint word addresses make the store order immaterial. Emitted-`.s`-level
// (machine translation-validation via opt-parity + torture), NOT IR `equiv` — it is a
// pure output rewrite the backend model already trusts (like the move peephole).
//
// IMPROVEMENT (static, no race): the memory-op count HALVES on every run of ≥2
// adjacent same-base accesses — the callee-save slab (every non-leaf function),
// struct copies, HFA/param spills.
//
// SOUNDNESS FENCES (each a constrained-unpredictable/aliasing hazard avoided):
//   • same load/store direction, same register class (x/w/d/s), same base symbol;
//   • the second offset is EXACTLY first + sz, and `o` is a legal scaled imm7
//     (multiple of sz, o/sz ∈ [-64,63]);
//   • the base register is not one of the two transferred GP/W registers (its value
//     must survive to address the pair — a `ldr xBase,[xBase,..]` mustn't be paired);
//   • `ldp` forbids the two destinations being identical.
// Only plain `ldr`/`str` (full-width, non-extending) parse; `ldrb`/`ldrsw`/`q`-regs
// are skipped (different scaling / no pairing form).
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `str|ldr {x|w|d|s}N, [<base>[, #<off>]]` → (is_load, class byte, reg#, base, off).
fn parse_ldst(line: &str) -> Option<(bool, u8, u32, String, i64)> {
    let t = line.trim();
    let (is_load, rest) = if let Some(r) = t.strip_prefix("ldr ") {
        (true, r)
    } else if let Some(r) = t.strip_prefix("str ") {
        (false, r)
    } else {
        return None;
    };
    let (reg_s, mem) = rest.split_once(", [")?;
    let mem = mem.strip_suffix(']')?;
    let cls = reg_s.as_bytes().first().copied()?;
    if !matches!(cls, b'x' | b'w' | b'd' | b's') {
        return None;
    }
    let reg: u32 = reg_s.get(1..)?.parse().ok()?;
    let (base, off) = match mem.split_once(", #") {
        Some((b, o)) => (b.to_string(), o.parse::<i64>().ok()?),
        None => (mem.to_string(), 0),
    };
    Some((is_load, cls, reg, base, off))
}

/// Fuse consecutive adjacent accesses into `ldp`/`stp`. Runs AFTER the move peephole
/// (which may delete lines between two accesses, exposing the adjacency).
fn pair_ldst(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if i + 1 < lines.len() {
            if let (Some((la, ca, ra, ba, oa)), Some((lb, cb, rb, bb, ob))) =
                (parse_ldst(lines[i]), parse_ldst(lines[i + 1]))
            {
                let sz: i64 = if ca == b'x' || ca == b'd' { 8 } else { 4 };
                let scaled = oa % sz == 0 && (oa / sz) >= -64 && (oa / sz) <= 63;
                // an x/w transfer register aliases the 64-bit x base; d/s live in the
                // separate FP file and never clash.
                let base_clash = matches!(ca, b'x' | b'w')
                    && (ba == format!("x{ra}") || ba == format!("x{rb}"));
                if la == lb
                    && ca == cb
                    && ba == bb
                    && ob == oa + sz
                    && scaled
                    && !base_clash
                    && !(la && ra == rb) // ldp destinations must differ
                {
                    let mn = if la { "ldp" } else { "stp" };
                    let c = ca as char;
                    let addr = if oa == 0 { format!("[{ba}]") } else { format!("[{ba}, #{oa}]") };
                    _ = writeln!(out, "\t{mn} {c}{ra}, {c}{rb}, {addr}");
                    i += 2;
                    continue;
                }
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// DEAD-MOVE ELIMINATION (region-local backward liveness). A `mov xD,xS` is deleted when xD
/// is redefined later in the same straight-line region BEFORE any read of xD — its value is
/// never observed. The coalescer gives many short-lived temps the same home register, so the
/// emitter stores each to that home and overwrites it before it is read: pure dead stores.
/// Live-out at a region boundary is the conservative FULL set, so a move is dropped only when
/// a later write within the region provably kills it. Only `mov x,x` lines are ever removed.
fn drop_dead_moves(body: &str) -> String {
    use std::collections::HashSet;
    let lines: Vec<&str> = body.lines().collect();
    let mut drop = vec![false; lines.len()];
    let full: HashSet<u32> = (0..=30).collect();
    let mut live = full.clone();
    for (i, line) in lines.iter().enumerate().rev() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('.') {
            continue; // directive/blank: no effect on register liveness
        }
        if t.ends_with(':') {
            live = full.clone(); // label = region boundary above it ⟹ all live-out
            continue;
        }
        let (reads, writes, boundary) = reg_uses(t);
        if boundary {
            live = full.clone(); // branch/call/ret/unknown ⟹ conservative live-out
            continue;
        }
        if let Some((d, _s)) = parse_mov_xx(t) {
            if !live.contains(&d) {
                drop[i] = true; // xD is dead here ⟹ this store is never observed
                continue; // deleted ⟹ it neither reads nor writes
            }
        }
        for w in &writes {
            if !reads.contains(w) {
                live.remove(w); // a pure write kills the register above it
            }
        }
        for r in &reads {
            live.insert(*r);
        }
    }
    let mut out = String::with_capacity(body.len());
    for (i, line) in lines.iter().enumerate() {
        if !drop[i] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Redundant round-trip elimination via per-region value-equivalence (see the block comment).
fn drop_redundant_moves(body: &str) -> String {
    use std::collections::HashMap;
    let mut out = String::with_capacity(body.len());
    let mut eq: HashMap<u32, u64> = HashMap::new(); // register slot → value id
    let mut next: u64 = 0;
    // Recognized destination-writing mnemonics (dst = first register operand). Everything
    // NOT here and NOT a store/compare/branch flushes the model (conservative = safe).
    const DEF_FIRST: &[&str] = &[
        "mov", "movk", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn",
        "and", "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth",
        "sxtb", "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr",
        "ldrb", "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd",
        "fsub", "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl",
    ];
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if t.ends_with(':') {
            eq.clear(); // label = basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with('.') {
            out.push_str(line); // directive — touches no register
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        // The one rewrite: drop a mov xD,xS proven redundant; else record D≡S.
        if let Some((d, s)) = parse_mov_xx(t) {
            match (eq.get(&d), eq.get(&s)) {
                (Some(a), Some(b)) if a == b => continue, // D already ≡ S → DROP
                _ => {
                    let sid = *eq.entry(s).or_insert_with(|| {
                        next += 1;
                        next
                    });
                    eq.insert(d, sid);
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
            }
        }
        if mn == "ldp" {
            // two destinations = the first two register operands.
            let mut regs = operands.split(',');
            for _ in 0..2 {
                if let Some(r) = regs.next().and_then(|tok| {
                    let tok = tok.trim();
                    tok.strip_prefix('x').or_else(|| tok.strip_prefix('w'))?.parse::<u32>().ok()
                }) {
                    next += 1;
                    eq.insert(r, next);
                }
            }
        } else if NO_DEF.contains(&mn) {
            // no register destination — model unchanged.
        } else if DEF_FIRST.contains(&mn) {
            if let Some(r) = first_reg_slot(operands) {
                next += 1;
                eq.insert(r, next); // destination takes a fresh value ⟹ breaks stale ≡
            }
        } else {
            eq.clear(); // unrecognized (incl. b/bl/br/ret/cbz/…) ⟹ flush = safe
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// REDUNDANT-LOAD-AFTER-STORE elimination (store→load identity). [MEASURED lever:
// sqlite3.c carries 166,019 adjacent `str xN,[sp,#m]; ldr xN,[sp,#m]` pairs = 52% of
// ALL 319k loads — the value-contract materializes each temp to/from its frame slot per
// use (O0 style), and the register allocator's spill code round-trips through the slot;
// neither is visible to the IR-level load-elim (B2, §3), so they survive to the stream.
// This is the machine-level case of a veteran-compiler pass: GCC `postreload-cse`/
// `peephole2`, LLVM MachineCSE + store→load forwarding, QBE load.c.]
//
// THEOREM (store→load identity). State Σ=⟨ρ registers, μ memory⟩:
//   ⟦str xN,[m]⟧ : μ' = μ[addr(m) ↦ ρ(xN)], ρ unchanged
//   ⟦ldr xN,[m]⟧ : ρ' = ρ[xN ↦ μ(addr(m))], μ unchanged
// When the two are ADJACENT (nothing executes between ⟹ ρ(xN), the base register of m,
// and μ(addr(m)) are unperturbed), after `str` we have μ(addr(m)) = ρ(xN); then `ldr`
// assigns ρ(xN) := μ(addr(m)) = ρ(xN) — the IDENTITY on ρ. So ⟦str;ldr⟧ = ⟦str⟧ and
// deleting the `ldr` preserves ⟦·⟧. ∎  Full 64-bit `x` form only: a `w`-form reload
// zero-extends into the high 32 bits, an OBSERVABLE change unless those bits are already
// dead — that proof is not local, so `w` pairs are left untouched (there are none here).
//
// TWO HYPOTHESES OF THE THEOREM, both discharged by construction:
//   (1) NON-VOLATILE m — a volatile access must not be elided (C11 6.7.3/7: both the store
//       and the load are required observable side effects). The base is restricted to `[sp,`
//       (frame slots): sp is never a user pointer, so a frame slot is a compiler-generated
//       stack temp — never volatile, never aliased. (Measured: all 166,019 pairs are `[sp,`.)
//   (2) ADJACENCY with no control entry — a LABEL between the pair is an entry point at which
//       execution may reach the `ldr` WITHOUT having run the `str`, so μ(addr(m)) ≠ ρ(xN)
//       there. A label FLUSHES the pending store. Blank/directive lines carry no execution
//       and no entry, so the pair survives across them; any other instruction flushes (safe).
// SOUND like the move passes: the ONLY rewrite is deleting a proven-identity load; every
// value/memory-changing event drops the pending store. Re-validated by opt-parity (0 DIVERGE).

/// Parse a 64-bit `ldr`/`str xN, [sp, #k]` frame-slot access → (is_load, N, mem-text).
/// None for any other mnemonic, a `w`-form, a non-`[sp,` base, or writeback/index addressing.
fn parse_frame_ldst(t: &str) -> Option<(bool, u32, &str)> {
    let (mn, rest) = t.split_once(char::is_whitespace)?;
    let is_load = match mn {
        "ldr" => true,
        "str" => false,
        _ => return None,
    };
    let rest = rest.trim_start();
    // writeback (`[sp,#k]!`) / post-index (`[sp],#k`) mutate sp — not a pure load/store.
    if rest.contains('!') || rest.contains("],") {
        return None;
    }
    let (reg, mem) = rest.split_once(',')?;
    let n = reg.trim().strip_prefix('x')?.parse::<u32>().ok()?; // x-form (64-bit) only
    let mem = mem.trim();
    mem.starts_with("[sp,").then_some((is_load, n, mem))
}

/// Delete every `ldr xN,[sp,#m]` immediately preceded by `str xN,[sp,#m]` (store→load
/// identity, see the block comment). Airtight and value-independent; the single largest
/// measured reduction in the load stream.
fn drop_redundant_loads(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut pending_store: Option<(u32, String)> = None; // (reg, mem) of the last store
    for line in body.lines() {
        let t = line.trim();
        if t.ends_with(':') {
            // label (incl. local `.L…:`) = control-flow entry ⟹ store→load identity breaks.
            // Checked BEFORE the `.`-directive case, since local labels start with a dot.
            pending_store = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.is_empty() || t.starts_with('.') {
            out.push_str(line); // blank/directive: no execution, no control entry — keep pair
            out.push('\n');
            continue;
        }
        match parse_frame_ldst(t) {
            Some((true, reg, mem)) => {
                if pending_store.as_ref().is_some_and(|(sr, sm)| *sr == reg && sm == mem) {
                    pending_store = None; // the redundant reload — DROP it (not emitted)
                    continue;
                }
                pending_store = None; // a load that redefines xN: no store now pends
            }
            Some((false, reg, mem)) => pending_store = Some((reg, mem.to_string())),
            None => pending_store = None, // any other instruction may touch mem/regs ⟹ flush
        }
        out.push_str(line);
        out.push('\n');
    }
    out
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
    let opt_ok: Vec<bool> = ast
        .funcs
        .iter()
        .map(|f| !zcc_o0 && !f.has_volatile)
        .collect();
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
            crate::opt::inline(&ast.tt, &mut funcs, &caller_ok, &opt_ok);
        }
        for (i, f) in funcs.iter_mut().enumerate() {
            if opt_ok.get(i).copied().unwrap_or(false) {
                crate::opt::optimize_ssa(&ast.tt, f, &passes, GP_BUDGET.k);
                debug_assert!(ir::verify(f).is_ok(), "opt produced broken IR: {}", f.name);
            }
        }
    }
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
        csave_gp: Vec::new(),
        csave_fp: Vec::new(),
        use_count: Vec::new(),
        sp_at_base: true,
        fdynstack: false,
        near_branch: false,
    };
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n";
    }
    for (fi, f) in ast.funcs.iter().enumerate() {
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
        // Prologue parameter-ABI SHARED with emit() (nested-chain/variadic-save/sret/
        // spill scalar+struct+HFA) → parameters sit ready in frame slots for the IR body.
        emit_params(&mut g, f);
        let body_start = g.s.len();
        g.emit_ir_body(&funcs[fi]);
        // Phase C — machine-level cleanup over just this body (the region begins fresh:
        // entered from the prologue, so an empty equivalence model is sound).
        let mut body = g.s.split_off(body_start);
        // Redundant-load-after-store (store→load identity) matches only `[sp,#k]` frame slots,
        // which hold compiler temps — a program volatile object is accessed through an address
        // register (`[xN]`), never this form. That invariant holds because volatile objects live
        // only in `has_volatile` (→ -O0) functions, whose local Var accesses are not sp-folded.
        // We still gate on `!has_volatile` so the pass's soundness is LOCAL (no reliance on a
        // cross-pass fold invariant) — volatile-free functions, including at -O0, keep the win.
        if !f.has_volatile {
            body = drop_redundant_loads(&body); // run on the raw stream, before copy-prop renames
        }
        if g.regalloc && passes.peephole {
            body = peephole_moves(&body); // redundant/dead reg-moves…
        }
        if g.regalloc && passes.ldst_pair {
            body = pair_ldst(&body); // …then the exposed adjacent accesses → ldp/stp
        }
        g.s.push_str(&body);
        g.s += "\t.cfi_endproc\n";
        _ = writeln!(g.s, "\t.size {0}, .-{0}", f.name);
    }
    emit_module_tail(&mut g, ast);
    g.s
}

#[cfg(test)]
mod tests {
    use super::{is_logical_imm, pair_ldst, peephole_moves};

    fn count(s: &str, needle: &str) -> usize {
        s.lines().filter(|l| l.trim().starts_with(needle)).count()
    }

    // ARMv8 logical-immediate encodability (Side-II). Valid = rotation of a contiguous
    // ones-run replicated at a power-of-two element size; all-0 / all-1 invalid.
    #[test]
    fn logical_imm_encoding() {
        // valid patterns
        assert!(is_logical_imm(0xFF)); // 8 low ones (size 64, ones 8)
        assert!(is_logical_imm(0x1)); // single bit
        assert!(is_logical_imm(0x8000_0000)); // single bit high
        assert!(is_logical_imm(0xFFFF_FFFF_FFFF_FFFE)); // one zero bit (rotated run)
        assert!(is_logical_imm(0xF0F0_F0F0_F0F0_F0F0)); // element size 8, replicated
        assert!(is_logical_imm(0x5555_5555_5555_5555)); // element size 2, replicated
        assert!(is_logical_imm(0xFFFF_0000_FFFF_0000)); // element size 32
        assert!(is_logical_imm(0x0000_FFFF_0000_FFFF));
        // invalid patterns
        assert!(!is_logical_imm(0)); // all zeros
        assert!(!is_logical_imm(u64::MAX)); // all ones
        assert!(!is_logical_imm(0x3 | 0x18)); // 0b11011 — two runs, not a single rotated run
        assert!(!is_logical_imm(0xFF00_FF00_FF00_0000)); // not uniformly replicated
    }

    // B4 ldp/stp — the callee-save slab pattern: consecutive same-base 8-byte stores fuse.
    #[test]
    fn pair_fuses_callee_save_slab() {
        let body = "\tstr x23, [x9, #0]\n\tstr x19, [x9, #8]\n\tstr x20, [x9, #16]\n\tstr x21, [x9, #32]\n";
        let out = pair_ldst(body);
        // (0,8)→stp, (16) has no #24 partner (next is #32) → stays str; #32 alone.
        assert_eq!(count(&out, "stp"), 1, "one pair formed");
        assert!(out.contains("stp x23, x19, [x9]"), "first two paired: {out}");
        assert_eq!(count(&out, "str"), 2, "the two unpaired stores remain");
    }

    #[test]
    fn pair_fuses_ldp_and_offsets() {
        let out = pair_ldst("\tldr x20, [x9, #16]\n\tldr x21, [x9, #24]\n");
        assert!(out.contains("ldp x20, x21, [x9, #16]"), "{out}");
    }

    // SOUNDNESS fences — none of these may fuse.
    #[test]
    fn pair_respects_fences() {
        // non-adjacent offsets (#0 then #16, gap of 16 ≠ 8)
        assert!(!pair_ldst("\tstr x0, [x9, #0]\n\tstr x1, [x9, #16]\n").contains("stp"));
        // different base
        assert!(!pair_ldst("\tstr x0, [x9, #0]\n\tstr x1, [x10, #8]\n").contains("stp"));
        // mixed class (x then d)
        assert!(!pair_ldst("\tstr x0, [x9, #0]\n\tstr d1, [x9, #8]\n").contains("stp"));
        // ldp base clash: ldr into the base register
        assert!(!pair_ldst("\tldr x9, [x9, #0]\n\tldr x1, [x9, #8]\n").contains("ldp"));
        // ldp identical destinations
        assert!(!pair_ldst("\tldr x0, [x9, #0]\n\tldr x0, [x9, #8]\n").contains("ldp"));
        // misaligned scaled offset (#4 for an 8-byte x access)
        assert!(!pair_ldst("\tstr x0, [x9, #4]\n\tstr x1, [x9, #12]\n").contains("stp"));
        // mixed direction (str then ldr)
        assert!(!pair_ldst("\tstr x0, [x9, #0]\n\tldr x1, [x9, #8]\n").contains("stp"));
    }

    // The core case: `mov xH, x0` then `mov x0, xH` — the second reload is redundant
    // (x0 already holds xH's value) and must be DROPPED; the first store must be KEPT.
    #[test]
    fn peephole_drops_redundant_roundtrip() {
        let body = "\tmov x24, x0\n\tmov x0, x24\n\tadd x0, x0, x1\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 0, "the redundant reload must be dropped");
        assert_eq!(count(&out, "mov x24, x0"), 1, "the store to the home must be kept");
        assert!(out.contains("add x0, x0, x1"), "the real op is untouched");
    }

    // A DEF between the two movs BREAKS the equivalence — the reload is NOT redundant and
    // must be KEPT (x0 was clobbered by the mul).
    #[test]
    fn peephole_keeps_move_after_clobber() {
        let body = "\tmov x24, x0\n\tmul x0, x5, x6\n\tmov x0, x24\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 1, "x0 was clobbered ⟹ the reload is real");
    }

    // A label (basic-block boundary) FLUSHES the model — a cross-boundary equivalence must
    // never be assumed (the predecessor might not have set it).
    #[test]
    fn peephole_flushes_at_label() {
        let body = "\tmov x24, x0\n.Lx:\n\tmov x0, x24\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 1, "must not elide across a label");
    }

    // An UNRECOGNIZED mnemonic flushes conservatively (safety over coverage).
    #[test]
    fn peephole_flushes_on_unknown() {
        let body = "\tmov x24, x0\n\tzzz x0, x1\n\tmov x0, x24\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 1, "unknown insn ⟹ flush ⟹ keep the reload");
    }

    // Round-trip `mov x24,x0; mov x0,x24` is redundant and dropped; a genuinely distinct
    // move (different value) is preserved.
    #[test]
    fn peephole_preserves_distinct_move() {
        let body = "\tmov x24, x0\n\tmov x0, x24\n\tmov x1, x25\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 0, "redundant dropped");
        assert_eq!(count(&out, "mov x1, x25"), 1, "an unrelated move is preserved");
    }

    use super::drop_redundant_loads;

    // CORE store→load identity: `str x0,[sp,#24]; ldr x0,[sp,#24]` adjacent ⟹ the ldr is
    // the identity on x0 and must be DROPPED; the store is KEPT (a later block may reload it).
    #[test]
    fn redundant_load_after_store_dropped() {
        let body = "\tstr x0, [sp, #24]\n\tldr x0, [sp, #24]\n\tadd x0, x0, x1\n";
        let out = drop_redundant_loads(body);
        assert_eq!(count(&out, "ldr x0, [sp, #24]"), 0, "the redundant reload is deleted");
        assert_eq!(count(&out, "str x0, [sp, #24]"), 1, "the store is kept");
        assert!(out.contains("add x0, x0, x1"), "the real op is untouched");
    }

    // A DIFFERENT destination register is a real move (store→load forward into x1), NOT the
    // identity — it must be KEPT (we delete only the same-register no-op).
    #[test]
    fn redundant_load_diff_reg_kept() {
        let out = drop_redundant_loads("\tstr x0, [sp, #24]\n\tldr x1, [sp, #24]\n");
        assert_eq!(count(&out, "ldr x1, [sp, #24]"), 1, "load into a distinct reg is not a no-op");
    }

    // A DIFFERENT slot is a genuine load — KEPT.
    #[test]
    fn redundant_load_diff_slot_kept() {
        let out = drop_redundant_loads("\tstr x0, [sp, #24]\n\tldr x0, [sp, #32]\n");
        assert_eq!(count(&out, "ldr x0, [sp, #32]"), 1, "a different slot is a real load");
    }

    // Hypothesis (2): a LABEL between the pair is a control entry point ⟹ the load may run
    // without the store ⟹ NOT redundant. Must be KEPT.
    #[test]
    fn redundant_load_flushed_at_label() {
        let out = drop_redundant_loads("\tstr x0, [sp, #24]\n.Lx:\n\tldr x0, [sp, #24]\n");
        assert_eq!(count(&out, "ldr x0, [sp, #24]"), 1, "must not elide across a label");
    }

    // Any intervening instruction (it may write memory or the register) FLUSHES the pending
    // store ⟹ the reload is real. Must be KEPT.
    #[test]
    fn redundant_load_flushed_by_intervening_insn() {
        let out = drop_redundant_loads("\tstr x0, [sp, #24]\n\tmul x0, x5, x6\n\tldr x0, [sp, #24]\n");
        assert_eq!(count(&out, "ldr x0, [sp, #24]"), 1, "clobber between ⟹ the reload is real");
    }

    // Hypothesis (1): a NON-frame base (`[x9]`) may alias a VOLATILE object — the restriction
    // to `[sp,` excludes it, so such a pair is left untouched.
    #[test]
    fn redundant_load_nonframe_base_kept() {
        let out = drop_redundant_loads("\tstr x0, [x9]\n\tldr x0, [x9]\n");
        assert_eq!(count(&out, "ldr x0, [x9]"), 1, "arbitrary-pointer pairs are never elided");
    }

    // A `w`-form reload zero-extends the high 32 bits (an observable change unless dead) ⟹ NOT
    // the 64-bit identity. Left untouched (x-form only).
    #[test]
    fn redundant_load_wform_kept() {
        let out = drop_redundant_loads("\tstr w0, [sp, #24]\n\tldr w0, [sp, #24]\n");
        assert_eq!(count(&out, "ldr w0, [sp, #24]"), 1, "w-form reload is not the 64-bit identity");
    }

    // A blank/directive line carries no execution and no control entry ⟹ the pair survives it.
    #[test]
    fn redundant_load_survives_directive() {
        let out = drop_redundant_loads("\tstr x0, [sp, #24]\n\t.p2align 3\n\tldr x0, [sp, #24]\n");
        assert_eq!(count(&out, "ldr x0, [sp, #24]"), 0, "a directive does not break adjacency");
    }

    use super::drop_dead_moves;

    // DEAD STORE: `mov x24, x0` then x24 is overwritten (`mov x24, x1`) before any read →
    // the first store is dead and must be removed; the live second store stays.
    #[test]
    fn dce_drops_dead_store() {
        let body = "\tmov x24, x0\n\tmov x24, x1\n\tmov x2, x24\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x24, x0"), 0, "the overwritten-before-read store is dead");
        assert_eq!(count(&out, "mov x24, x1"), 1, "the store that IS read must stay");
    }

    // TEETH: a `mov x24, x0` whose value IS read before any overwrite must NOT be dropped —
    // deleting it would lose the value. Guards against over-eager DCE (a miscompile).
    #[test]
    fn dce_keeps_used_store() {
        let body = "\tmov x24, x0\n\tmov x2, x24\n\tmov x24, x1\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x24, x0"), 1, "x24 is READ before overwrite ⟹ live, keep");
    }

    // A read INSIDE a compare/store counts — `str x24,[x1]` reads x24, so the prior store is live.
    #[test]
    fn dce_counts_reads_in_stores() {
        let body = "\tmov x24, x0\n\tstr x24, [x1]\n\tmov x24, x2\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x24, x0"), 1, "str reads x24 ⟹ the store is live");
    }

    // A region boundary (branch) means all registers are conservatively live-out — a store
    // with no in-region overwrite before the branch must be KEPT (it may be read by a successor).
    #[test]
    fn dce_conservative_across_boundary() {
        let body = "\tmov x24, x0\n\tcbz x1, .Lx\n\tmov x24, x2\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x24, x0"), 1, "live-out across a branch ⟹ keep");
    }

    // Writeback addressing implicitly mutates a base register — treated as a boundary, so no
    // move is dropped across it (safety over coverage).
    #[test]
    fn dce_safe_on_writeback() {
        let body = "\tmov x24, x0\n\tldr x2, [x3, #8]!\n\tmov x24, x1\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x24, x0"), 1, "writeback ⟹ boundary ⟹ conservative keep");
    }

    // REGRESSION (the stdarg-1 miscompile): a FLOAT/VECTOR destination whose ADDRESS is a GP
    // register — `ldr q0, [x0]` READS x0, it does NOT write it. The `mov x0, xS` feeding the
    // address must be KEPT (earlier a positional-parse bug mistook x0 for the destination and
    // dropped it, corrupting the load address → SIGABRT).
    #[test]
    fn dce_keeps_addr_of_float_load() {
        let body = "\tmov x0, x10\n\tldr q0, [x0]\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x0, x10"), 1, "x0 is the load address (read) ⟹ keep");
    }

    // Same class: `fmov d0, x0` READS x0 (int→float bitcast), does not write it.
    #[test]
    fn dce_keeps_src_of_fmov_to_float() {
        let body = "\tmov x0, x10\n\tfmov d0, x0\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x0, x10"), 1, "x0 is the fmov source (read) ⟹ keep");
    }

    // The converse must still work: `fmov x0, d0` WRITES x0 (float→int), so a prior dead store
    // to x0 IS dead and removable.
    #[test]
    fn dce_float_to_gp_writes_dst() {
        let body = "\tmov x0, x10\n\tfmov x0, d0\n\tmov x1, x0\n";
        let out = drop_dead_moves(body);
        assert_eq!(count(&out, "mov x0, x10"), 0, "fmov x0,d0 overwrites x0 ⟹ prior store dead");
    }
}
