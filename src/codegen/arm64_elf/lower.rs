//! Value-contract lowering: params in/out (AAPCS64), module tail (globals/rodata/TLS),
//! and the scalar value-contract impl (load/store/cast/ext/imm/lea_local). Side-I
//! algorithm; methods pub(super) so emit.rs + the emit_ir spine cross-call them.
use super::{Cg, ExtFold, ParamLoc};
use super::encoding::{sym, wr, xr};
use crate::ast::{Ast, GInit, Ty, TypeId};
use crate::ir::{Inst, Op, Place, Val};
use std::fmt::Write;

pub(super) fn emit_params(g: &mut Cg, f: &crate::ast::Func) {
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
pub(super) fn emit_module_tail(g: &mut Cg, ast: &Ast) {
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
    pub(super) fn gdata(&mut self, init: &GInit, sz: u32) {
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
    pub(super) fn store_narrow(&mut self, mut off: u32, mut sz: u32) {
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
    pub(super) fn labels(&mut self, k: u32) -> u32 {
        let n = self.lbl;
        self.lbl += k;
        n
    }
    pub(super) fn imm(&mut self, reg: &str, v: i64) {
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
    pub(super) fn sp_slot(&self, off: u32) -> Option<u32> {
        self.sp_slot_sz(off, 8)
    }
    // Size-parametric form (for the local addressing-fold, whose access width is 1/2/4/8).
    // pos must satisfy the ldr/str unsigned-scaled encoding: a multiple of the access size,
    // 0..=size·4095. A misaligned or out-of-range local keeps the two-instruction lea form.
    pub(super) fn sp_slot_sz(&self, off: u32, sz: u32) -> Option<u32> {
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
    pub(super) fn sp_add_slot(&self, off: u32) -> Option<u32> {
        if self.fhasvla || self.fdynstack || !self.sp_at_base {
            return None;
        }
        let total = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
        let pos = total.checked_sub(off)?;
        (pos <= 4095).then_some(pos)
    }
    // reg = x29 - off (off may exceed imm12)
    pub(super) fn lea_local(&mut self, reg: &str, off: u32) {
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
    pub(super) fn sp_adjust(&mut self, op: &str, n: u32) {
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
    pub(super) fn reset_sp_base(&mut self) {
        let off = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
        self.lea_local("x9", off);
        _ = writeln!(self.s, "\tmov sp, x9");
    }
    // Re-canonicalize x0 per type (after a 32-bit op / narrowing). Funnel default.
    pub(super) fn ext(&mut self, t: TypeId) {
        self.ext_r(0, t);
    }
    // Register-parametric re-canonicalization: x{r} = canon(x{r}) per the declared width
    // read from TyTab. r=0 is the x0-funnel default; the compute-into-home path (Tier-1 #1)
    // passes the destination's HOME register so the extension lands in place, no x0 detour.
    // Byte-identical to the old `ext` when r=0 (verified: same mnemonics, same order).
    pub(super) fn ext_r(&mut self, r: u32, t: TypeId) {
        self.ext_rd(r, r, t);
    }
    // Cast-and-relocate in one step: x{rd} = canon(x{ra}) per width `t`. The ARMv8 extend/
    // extract forms all take a distinct source register (`sxtw x{rd}, w{ra}`), so an integer
    // width-cast whose result is register-homed lands directly in the home with NO x0 funnel
    // (kills both `mov x0,aHome` and `mov dHome,x0`). ext_r is the rd==ra special case.
    pub(super) fn ext_rd(&mut self, rd: u32, ra: u32, t: TypeId) {
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
    pub(super) fn load(&mut self, t: TypeId) {
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
    pub(super) fn load_gp(&mut self, rd: u32, ra: u32, t: TypeId) {
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
    pub(super) fn simple_gp_load_ty(&self, t: TypeId) -> bool {
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
    pub(super) fn simple_gp_store_ty(&self, t: TypeId) -> bool {
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
    pub(super) fn is_addr_arith(&self, ct: TypeId) -> bool {
        matches!(self.a.tt.tys[ct as usize], Ty::Ptr(_) | Ty::Array(..)) || self.a.tt.size(ct) == 8
    }
    pub(super) fn load_gp_sp(&mut self, rd: u32, pos: u32, t: TypeId) {
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
    pub(super) fn store_gp_sp(&mut self, rv: u32, pos: u32, t: TypeId) {
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
    pub(super) fn load_gp_fp(&mut self, rd: u32, off: u32, t: TypeId) {
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
    pub(super) fn store_gp_fp(&mut self, rv: u32, off: u32, t: TypeId) {
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
    pub(super) fn load_gp_off(&mut self, rd: u32, rbase: u32, off: u32, t: TypeId) {
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
    pub(super) fn store_gp_off(&mut self, rv: u32, rbase: u32, off: u32, t: TypeId) {
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
    pub(super) fn scaled_off(&self, off: u32, size: u32) -> bool {
        size != 0 && off % size == 0 && off / size <= 4095
    }
    // Tier-1 #2 — register-offset load: x{rd} = *(x{rbase} + x{rindex}), width per t. The
    // ARM64 `[Xn, Xm]` addressing form adds the full 64-bit Xm; it exists for every ldr
    // variant used here (ldr/ldrb/ldrh/ldrsb/ldrsh/ldrsw). rd may alias rbase/rindex (base
    // and index are read before rd is written — a single instruction).
    pub(super) fn load_idx(&mut self, rd: u32, rbase: u32, rindex: u32, t: TypeId) {
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
    pub(super) fn ext_suffix(f: &ExtFold) -> String {
        let e = if f.signed { "sxtw" } else { "uxtw" };
        if f.shift == 0 { format!(", {e}") } else { format!(", {e} #{}", f.shift) }
    }
    pub(super) fn load_idx_ext(&mut self, rd: u32, f: &ExtFold, t: TypeId) {
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
    pub(super) fn store_idx_ext(&mut self, rv: u32, f: &ExtFold, t: TypeId) {
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
    pub(super) fn store_idx(&mut self, rv: u32, rbase: u32, rindex: u32, t: TypeId) {
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
    pub(super) fn try_fuse_addr(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
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
    pub(super) fn try_fuse_store_addr(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
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
    pub(super) fn try_fuse_madd(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
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
    pub(super) fn try_fuse_local(&mut self, insts: &[Inst], i: usize) -> Option<usize> {
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
    pub(super) fn store_z(&mut self, t: TypeId) {
        let ad = self.fnl + 1; // funnel address (x1/x11)
        _ = match self.a.tt.size(t) {
            1 => writeln!(self.s, "\tstrb wzr, [x{ad}]"),
            2 => writeln!(self.s, "\tstrh wzr, [x{ad}]"),
            4 => writeln!(self.s, "\tstr wzr, [x{ad}]"),
            _ => writeln!(self.s, "\tstr xzr, [x{ad}]"),
        };
    }
    pub(super) fn store(&mut self, reg: u32, t: TypeId) {
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
    pub(super) fn blk_copy(&mut self, sz: u32) {
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
    pub(super) fn cast_op(&mut self, from: TypeId, to: TypeId) {
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
    pub(super) fn emit_funaddr(&mut self, name: &str) {
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
    pub(super) fn emit_zero(&mut self, sz: u32) {
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
    pub(super) fn emit_labeladdr(&mut self, name: &str) {
        let v = self.fnl;
        _ = writeln!(
            self.s,
            "\tadrp x{v}, lg_{0}.{1}\n\tadd x{v}, x{v}, :lo12:lg_{0}.{1}",
            self.fname, name
        );
    }
    // __builtin_*_overflow: x0=a, x1=b, x9=&res. bool result → x0. Shared by the
    // AST-walk (Node::Overflow) + IR (Inst::Overflow). ta/tb/rt = types of a/b/*rp.
    pub(super) fn emit_overflow(&mut self, op: u8, ta: TypeId, tb: TypeId, rt: TypeId) {
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
    pub(super) fn emit_vastart(&mut self) {
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
    pub(super) fn emit_vaarg(&mut self, t: TypeId, tmp: u32) {
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
