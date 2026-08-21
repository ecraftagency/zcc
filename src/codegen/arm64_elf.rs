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
use crate::opt::{AbiHome, ClassBudget};
use std::fmt::Write;

// Stage 5b — AAPCS64 §6.1.1 register files partitioned for `opt::abi_alloc`. The
// SPEC-TABLE side of the pass (the algorithm lives in opt.rs): a color index maps to a
// physical register here. A color ≥ ncaller is callee-saved ⟹ obliges a prologue/
// epilogue save/restore.
//   GP: NO caller-saved register is free — the emitter's scratch set spans x0–x15 across
//   BOTH arm64_elf.rs AND ext.rs (overflow_emit uses x14/x15), x16–x18 are ABI-reserved
//   — so the GP pool is exactly the callee-saved file x19–x28 (ncaller=0). (Measured the
//   hard way: pr64006 hung when x14/x15 were pooled — ext.rs was outside the first grep.)
//   FP: caller-saved v16–v31 then callee-saved v8–v15 (only d8–d15 preserved across a bl).
const GP_BUDGET: ClassBudget = ClassBudget { k: 10, ncaller: 0 };
const FP_BUDGET: ClassBudget = ClassBudget { k: 24, ncaller: 16 };
fn gp_phys(idx: u32) -> u32 {
    19 + idx // x19–x28 (ncaller=0 ⟹ every GP color is callee-saved)
}
fn fp_phys(idx: u32) -> u32 {
    if idx < FP_BUDGET.ncaller { 16 + idx } else { 8 + (idx - FP_BUDGET.ncaller) }
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
    csave_gp: Vec<u32>,
    csave_fp: Vec<u32>,
}


fn emit_params(g: &mut Cg, f: &crate::ast::Func) {
    let ast = g.a;
    if f.variadic {
        // AAPCS register-save area: spill ALL 8 q-regs + 8 x-regs (including the
        // named portion — harmless redundancy that avoids branching); must precede
        // parameter spilling (which reads the original registers)
        g.sp_adjust("sub", 192);
        g.imm("x9", (f.frame + 192) as i64);
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
            g.lea_local("x9", off);
            match ast.tt.size(t) {
                4 => _ = writeln!(g.s, "\tstr s{fp}, [x9]"),
                16 => _ = writeln!(g.s, "\tstr q{fp}, [x9]"), // long double: full binary128
                _ => _ = writeln!(g.s, "\tstr d{fp}, [x9]"),
            }
            fp += 1;
        } else if !fl && gp < 8 {
            g.lea_local("x9", off);
            _ = match ast.tt.size(t) {
                1 => writeln!(g.s, "\tstrb w{gp}, [x9]"),
                2 => writeln!(g.s, "\tstrh w{gp}, [x9]"),
                4 => writeln!(g.s, "\tstr w{gp}, [x9]"),
                _ => writeln!(g.s, "\tstr x{gp}, [x9]"),
            };
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
    g.va = (gp.min(8), fp.min(8), boff, f.frame);
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
    // reg = x29 - off (off may exceed imm12)
    fn lea_local(&mut self, reg: &str, off: u32) {
        if off <= 4095 {
            _ = writeln!(self.s, "\tsub {reg}, x29, #{off}");
        } else {
            self.imm("x10", off as i64);
            _ = writeln!(self.s, "\tsub {reg}, x29, x10");
        }
    }
    fn sp_adjust(&mut self, op: &str, n: u32) {
        if n <= 4095 {
            _ = writeln!(self.s, "\t{op} sp, sp, #{n}");
        } else {
            self.imm("x10", n as i64);
            _ = writeln!(self.s, "\t{op} sp, sp, x10");
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
    // Re-canonicalize x0 per type (after a 32-bit op / narrowing)
    fn ext(&mut self, t: TypeId) {
        if matches!(self.a.tt.tys[t as usize], Ty::Bool) {
            self.s += "\tcmp x0, #0\n\tcset x0, ne\n";
            return;
        }
        // Bitfield: truncate to w bits per the base's signedness — the value of (l.m = v)
        // is v AFTER truncation (GCC torture 921016-1)
        if let Ty::Bitfield(b, _, w) = self.a.tt.tys[t as usize] {
            let sh = 64 - w;
            let op = if self.a.tt.is_unsigned(b) {
                "lsr"
            } else {
                "asr"
            };
            _ = writeln!(self.s, "\tlsl x0, x0, #{sh}\n\t{op} x0, x0, #{sh}");
            return;
        }
        let u = self.a.tt.is_unsigned(t);
        self.s += match (self.a.tt.size(t), u) {
            (1, false) => "\tsxtb x0, w0\n",
            (1, true) => "\tuxtb w0, w0\n", // writing w → the upper 32 bits are auto-zeroed
            (2, false) => "\tsxth x0, w0\n",
            (2, true) => "\tuxth w0, w0\n",
            (4, false) => "\tsxtw x0, w0\n",
            (4, true) => "\tmov w0, w0\n",
            _ => return,
        };
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
    // store x{reg} → [x1] per type
    fn store(&mut self, reg: u32, t: TypeId) {
        match self.a.tt.tys[t as usize] {
            Ty::Bool => {
                _ = writeln!(
                    self.s,
                    "\tcmp x{reg}, #0\n\tcset x{reg}, ne\n\tstrb w{reg}, [x1]"
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
        self.ir_tbase + 8 + i * 8
    }
    // Stage 5b — a temp's home is a physical register (Chaitin color) or a spill slot.
    // `reg` is always a 64-bit GPR (verified: every call site passes an x-form); an
    // FP-homed temp holds the f64 bit pattern (SEMANTICS §1), moved via `fmov` GPR↔d-reg.
    fn tmp_load(&mut self, i: Tmp, reg: &str) {
        match self.talloc.get(i as usize).copied().flatten() {
            Some((true, idx)) => _ = writeln!(self.s, "\tfmov {reg}, d{}", fp_phys(idx)),
            Some((false, idx)) => _ = writeln!(self.s, "\tmov {reg}, x{}", gp_phys(idx)),
            None => {
                self.lea_local("x9", self.ir_toff(i));
                _ = writeln!(self.s, "\tldr {reg}, [x9]");
            }
        }
    }
    fn tmp_store(&mut self, i: Tmp, reg: &str) {
        match self.talloc.get(i as usize).copied().flatten() {
            Some((true, idx)) => _ = writeln!(self.s, "\tfmov d{}, {reg}", fp_phys(idx)),
            Some((false, idx)) => _ = writeln!(self.s, "\tmov x{}, {reg}", gp_phys(idx)),
            None => {
                self.lea_local("x9", self.ir_toff(i));
                _ = writeln!(self.s, "\tstr {reg}, [x9]");
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
    fn lea_global_x0(&mut self, name: &str, off: i64) {
        let (is_tls, is_got) = {
            let gl = self.a.globals.iter().find(|g| g.name.as_str() == name);
            (
                gl.is_some_and(|g| g.is_tls),
                gl.is_some_and(|g| g.is_extern || (self.a.pic && !g.is_static)),
            )
        };
        if is_tls {
            _ = writeln!(self.s, "\tmrs x0, tpidr_el0\n\tadd x0, x0, #:tprel_hi12:{name}, lsl #12\n\tadd x0, x0, #:tprel_lo12_nc:{name}");
        } else if is_got {
            _ = writeln!(self.s, "\tadrp x0, :got:{name}\n\tldr x0, [x0, :got_lo12:{name}]");
        } else {
            _ = writeln!(self.s, "\tadrp x0, {name}\n\tadd x0, x0, :lo12:{name}");
        }
        if off > 4095 {
            self.imm("x9", off);
            self.s += "\tadd x0, x0, x9\n";
        } else if off > 0 {
            _ = writeln!(self.s, "\tadd x0, x0, #{off}");
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
        let (mut gp, mut fp) = (0u32, 0u32);
        for &a in args {
            if self.val_is_float(a) && fp < 8 {
                self.ld_val(a, "x9");
                _ = writeln!(self.s, "\tfmov d{fp}, x9");
                fp += 1;
            } else if !self.val_is_float(a) && gp < 8 {
                let r = format!("x{gp}");
                self.ld_val(a, &r);
                gp += 1;
            }
            // TODO(expand): stack-overflow args (>8) + struct/HFA by value
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
            match self.a.tt.tys[rt as usize] {
                Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
                Ty::Double => self.s += "\tfmov x0, d0\n",
                Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n",
                Ty::Void | Ty::Struct(_) => {}
                _ => self.ext(rt),
            }
            self.tmp_store(*d, "x0");
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
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            // φ is an SSA-internal node; out_of_ssa (Stage 3) lowers every φ to copies
            // on the predecessor edges before codegen. Reaching the backend = a bug.
            Inst::Phi(..) => unreachable!("Inst::Phi must be eliminated by out_of_ssa before codegen"),
            Inst::Copy(d, _ty, a) => {
                self.ld_val(*a, "x0");
                self.tmp_store(*d, "x0");
            }
            Inst::Bin(d, op, ty, a, b) => {
                self.ld_val(*a, "x0");
                self.ld_val(*b, "x1");
                self.ir_bin(*op, *ty);
                self.tmp_store(*d, "x0");
            }
            Inst::Un(d, u, ty, a) => {
                self.ld_val(*a, "x0");
                match u {
                    Un::Neg if self.a.tt.is_float(*ty) => {
                        self.s += "\tfmov d0, x0\n\tfneg d0, d0\n\tfmov x0, d0\n"
                    }
                    Un::Neg => {
                        self.s += "\tneg x0, x0\n";
                        self.ext(*ty);
                    }
                    Un::BNot => {
                        self.s += "\tmvn x0, x0\n";
                        self.ext(*ty);
                    }
                }
                self.tmp_store(*d, "x0");
            }
            Inst::Load(d, ty, a) => {
                self.ld_val(*a, "x0");
                self.load(*ty);
                self.tmp_store(*d, "x0");
            }
            Inst::Store(ty, a, v) => {
                self.ld_val(*v, "x0");
                self.ld_val(*a, "x1");
                self.store(0, *ty);
            }
            Inst::Memcpy(d, s, sz) => {
                self.ld_val(*s, "x0"); // src
                self.ld_val(*d, "x1"); // dst
                self.blk_copy(*sz);
            }
            Inst::Lea(d, p) => {
                match p {
                    Place::Local(off) => self.lea_local("x0", *off),
                    Place::Global(name, off) => self.lea_global_x0(name, *off),
                    Place::Str(i) => _ = writeln!(
                        self.s,
                        "\tadrp x0, l_str{0}\n\tadd x0, x0, :lo12:l_str{0}",
                        i
                    ),
                }
                self.tmp_store(*d, "x0");
            }
            Inst::Cast(d, from, to, a) => {
                self.ld_val(*a, "x0");
                self.cast_op(*from, *to);
                self.tmp_store(*d, "x0");
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

    fn emit_term(&mut self, t: &Term) {
        match t {
            Term::Jmp(b) => _ = writeln!(self.s, "\tb {}", self.ir_label(*b)),
            Term::Br(c, tb, eb) => {
                self.ld_val(*c, "x0");
                let (lt, le) = (self.ir_label(*tb), self.ir_label(*eb));
                // Branch relaxation: cbnz/b.cond encode only a ±1MB displacement (imm19),
                // but the two IR block labels can sit arbitrarily far apart in a very large
                // function (e.g. -O0 output of an arithmetic fuzzer, hundreds of thousands
                // of instructions). Reach each target with an unconditional `b` (±128MB,
                // imm26), gated by a conditional skip to an ADJACENT local label that is
                // always within range. c!=0 → then; c==0 → skip to else.
                let n = self.labels(1);
                _ = writeln!(self.s, "\tcbz x0, L{n}\n\tb {lt}\nL{n}:\n\tb {le}");
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
        self.talloc = if self.regalloc {
            crate::opt::abi_alloc(&self.a.tt, irf, &GP_BUDGET, &FP_BUDGET, self.coalesce)
        } else {
            vec![None; irf.temps.len()]
        };
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
                Some((false, idx)) if idx >= GP_BUDGET.ncaller => {
                    let r = gp_phys(idx);
                    if !self.csave_gp.contains(&r) {
                        self.csave_gp.push(r);
                    }
                }
                _ => {}
            }
        }
        let tbytes = (irf.temps.len() as u32 * 8).next_multiple_of(16);
        let csave = ((self.csave_gp.len() + self.csave_fp.len()) as u32 * 8).next_multiple_of(16);
        self.ir_tspill = tbytes + csave; // reset_sp_base (VLA-dealloc) must also subtract this region
        if self.ir_tspill > 0 {
            self.sp_adjust("sub", self.ir_tspill);
        }
        self.save_callee(true); // spill callee-saved regs into the frame-bottom slab
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
            for inst in &blk.insts {
                self.emit_inst(inst);
            }
            self.emit_term(&blk.term);
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

/// Machine-level redundant-move elimination over one function body (see the block comment).
/// Chained with `drop_dead_moves` — redundant round-trips first, then dead stores.
fn peephole_moves(body: &str) -> String {
    drop_dead_moves(&drop_redundant_moves(body))
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

/// Backend entry point — the SOLE path: lower(AST) → IR → passes → asm. Covers the
/// full suite/csmith/musl; the AST-walk emit() has been removed. The backend simulates per-inst.
pub fn emit_ir(ast: &Ast) -> String {
    let mut funcs = ir::lower(ast);
    // Optimization is DEFAULT-ON on this branch (the ssa-qbe fork IS the optimizing
    // compiler): optimize_ssa = to_ssa ▸ sccp/gvn/const-fold/copy-prop/cse/dce ▸ out_of_ssa,
    // returning φ-free IR the naive-slot backend consumes unchanged. Every pass is proven
    // ⟦·⟧-preserving (opt.rs::tests, commuting-square); verify rejects broken IR.
    // Two guards, both MANDATORY (not A/B scaffolding):
    //   (1) has_volatile — the IR does not model volatile (6.7.3), so ⟦·⟧-preservation is
    //       proven only for volatile-free input; a volatile function keeps the naive -O0 path.
    //   (2) ZCC_O0 — the -O0 escape (debug + the bench baseline), the sole knob that turns
    //       the optimizer off. Default (unset) = full SSA optimization + regalloc.
    let opt_on = !ast.has_volatile && std::env::var("ZCC_O0").is_err();
    // Industrial toggleable pipeline: which passes run is read once from the environment
    // (ZCC_OPT_OFF / ZCC_OPT_ON over the default profile). `coalesce` is consumed later by
    // abi_alloc, so it is stashed on Cg.
    let passes = crate::opt::Passes::from_env();
    if opt_on {
        for f in funcs.iter_mut() {
            crate::opt::optimize_ssa(&ast.tt, f, &passes);
            debug_assert!(ir::verify(f).is_ok(), "opt produced broken IR: {}", f.name);
        }
    }
    // Stage 5b — ABI-aware regalloc runs whenever opt runs (φ-free, volatile-free IR);
    // off ⟹ the naive all-spill memory model (the -O0 baseline).
    let regalloc = opt_on;
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
        regalloc,
        coalesce: passes.coalesce,
        talloc: Vec::new(),
        csave_gp: Vec::new(),
        csave_fp: Vec::new(),
    };
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n";
    }
    for (fi, f) in ast.funcs.iter().enumerate() {
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
        g.fframe = f.frame;
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
        if f.frame > 0 {
            g.sp_adjust("sub", f.frame);
        }
        // Prologue parameter-ABI SHARED with emit() (nested-chain/variadic-save/sret/
        // spill scalar+struct+HFA) → parameters sit ready in frame slots for the IR body.
        emit_params(&mut g, f);
        let body_start = g.s.len();
        g.emit_ir_body(&funcs[fi]);
        // Phase C — machine-level redundant-move elimination over just this body (the region
        // begins fresh: entered from the prologue, so an empty equivalence model is sound).
        if passes.peephole && regalloc {
            let body = g.s.split_off(body_start);
            g.s.push_str(&peephole_moves(&body));
        }
        g.s += "\t.cfi_endproc\n";
        _ = writeln!(g.s, "\t.size {0}, .-{0}", f.name);
    }
    emit_module_tail(&mut g, ast);
    g.s
}

#[cfg(test)]
mod tests {
    use super::peephole_moves;

    fn count(s: &str, needle: &str) -> usize {
        s.lines().filter(|l| l.trim().starts_with(needle)).count()
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

    // Chained equivalence: x0≡x24≡x0 across a 3-hop still resolves; a genuinely distinct
    // move (different value) is preserved.
    #[test]
    fn peephole_preserves_distinct_move() {
        let body = "\tmov x24, x0\n\tmov x0, x24\n\tmov x1, x25\n";
        let out = peephole_moves(body);
        assert_eq!(count(&out, "mov x0, x24"), 0, "redundant dropped");
        assert_eq!(count(&out, "mov x1, x25"), 1, "an unrelated move is preserved");
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
