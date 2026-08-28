// MIR(final) → AArch64-ELF assembly text (MECHANISM.md §G9; THEORY II-4 for the
// THEORY II-4 — ELF sections and relocations; THEORY II-5 — A64 syntax
// ELF/relocation side).
//
// This file makes NO decisions. Every instruction was already chosen, every
// register already assigned, every offset already computed; printing is a
// one-to-one function from `MInst` to a line of text. That is the property the
// cost model depends on (MECHANISM.md §G10): `cost(f) = |MIR_final(f)|` needs no
// separate model precisely because nothing here expands, folds or fixes
// anything — the single exception is `MovImm`, whose `movz/movk` chain length
// `isa::mov_chain` reports before emission.
//
// It is also why rc3's disease cannot recur: there is no room for a peephole
// here. A machine optimization must be an MIR pass, where it can be verified.
use crate::ast::{Ast, GInit, Global};
use crate::mir::*;
use std::fmt::Write;

pub fn emit(ast: &Ast, m: &MModule) -> String {
    // an allocation hint only: nothing about the output depends on it
    let mut s = String::with_capacity(64 * 1024);
    // EXT(gcc): a top-level __asm__ is emitted verbatim, before anything else
    for a in &ast.raw_asm {
        let _ = writeln!(s, "{}", a);
    }
    s.push_str("\t.text\n");
    for f in &m.funcs {
        func(&mut s, ast, f);
    }
    data(&mut s, ast);
    // EXT(gcc): __attribute__((alias("old")))
    for (new, old, weak) in &ast.aliases {
        let _ = writeln!(
            s,
            "\t{} {}\n\t.set {}, {}",
            if *weak { ".weak" } else { ".globl" },
            sym_name(new),
            sym_name(new),
            sym_name(old)
        );
    }
    for w in &ast.weak_decls {
        let _ = writeln!(s, "\t.weak {}", sym_name(w));
    }
    // The same rule for a weak OBJECT declaration this TU only references:
    // `data()` skips an extern (it reserves no storage), so without this the
    // reference is a STRONG undefined one and a static link that has no
    // definition fails instead of resolving it to 0. musl's
    // `extern weak hidden const size_t _DYNAMIC[];` is exactly this, and the
    // `if (_DYNAMIC)` guarding it is written to read false when it is absent.
    for g in &ast.globals {
        if g.is_extern && g.is_weak {
            let _ = writeln!(s, "\t.weak {}", sym_name(&g.name));
        }
    }
    s
}

/// The parser marks a name whose spelling is final with a \x01 prefix. On ELF
/// no name gets a leading underscore anyway, so the prefix is simply stripped.
fn sym_name(n: &str) -> String {
    n.strip_prefix('\u{1}').unwrap_or(n).to_string()
}

/// THEORY II-4 — ELF section names
///
/// `-ffunction-sections`: each function goes in a section of its own,
/// `.text.<name>`, instead of all of them sharing `.text`. The linker can then
/// drop the ones nothing references (`--gc-sections`), which is why real
/// size-conscious builds pass this pair; the section name is the convention
/// every ELF toolchain already agrees on, so `ld` needs no help to place them.
///
/// It also makes a function individually REPLACEABLE at link time, which is
/// what turns "sqlite is 1.65x and we do not know which code carries it" into
/// a measurement: remove one `.text.<f>` from zcc's object, link it ahead of a
/// gcc-compiled object of the same source, and `f` — and only `f` — comes from
/// gcc. Attribution by linker rather than by profiler, on a box whose kernel
/// exposes no PMU.
pub fn function_sections() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ZCC_FUNCTION_SECTIONS").is_some())
}

/// The section a function's code belongs in.
fn text_section(name: &str) -> String {
    if function_sections() {
        format!("\t.section .text.{},\"ax\",@progbits\n\t.p2align 2\n", name)
    } else {
        "\t.text\n\t.p2align 2\n".to_string()
    }
}

// ── functions ──────────────────────────────────────────────────────────────
fn func(s: &mut String, ast: &Ast, f: &MFunc) {
    let name = sym_name(&f.name);
    s.push_str(&text_section(&name));
    if f.is_weak {
        let _ = writeln!(s, "\t.weak {}", name);
    } else if !f.is_static {
        let _ = writeln!(s, "\t.globl {}", name);
    }
    let _ = writeln!(s, "\t.type {}, %function\n{}:", name, name);
    // One frame adjustment, by construction (§8): nothing else moves sp — except
    // `StackAlloc`, and a function containing one takes a frame pointer so that
    // every static object keeps a fixed address regardless of where sp went.
    //
    // R4.15: for an ordinary (non-dynamic) frame the adjust is a real `SpAdj`
    // instruction placed by `frame_fold` — or folded into the first save pair —
    // so `emit` no longer invents it here. A dynamic frame keeps the printed form
    // because its adjust brackets the instant x29 stops being the caller's value.
    if f.frame_size > 0 && f.dyn_stack {
        adjust_sp(s, -(f.frame_size as i64));
    }
    if f.dyn_stack {
        // x29 is saved through sp, because at this instant it still holds the
        // CALLER's frame pointer — the one instruction pair that cannot be an
        // ordinary `Spill` (whose base would already be this frame's x29).
        let _ = writeln!(
            s,
            "\tstr x29, [sp, #{}]\n\tmov x29, sp",
            f.slots[f.fp_slot as usize].off
        );
    }
    let mut tables: Vec<(String, Vec<MBlockId>)> = Vec::new();
    let order: Vec<MBlockId> = if f.order.is_empty() {
        (0..f.blocks.len() as MBlockId).collect()
    } else {
        f.order.clone()
    };
    for (i, &b) in order.iter().enumerate() {
        let _ = writeln!(s, ".L{}_{}:", name, b);
        // C99 6.8.6.1: a `goto` may leave a VLA's scope, and the object must be
        // deallocated — sp returns to the frame base at every label. A jump INTO
        // a VLA scope is forbidden by the same clause, so the target's depth is
        // never greater than the current one and this is always safe.
        // EXT(gcc) `&&label`: a STATIC initializer may hold a label's address,
        // and the linker needs a symbol for it — the parser spells it
        // `lg_<function>.<label>` (parser.rs, `GInit::Addr`/`Diff`).
        for l in &f.blocks[b as usize].labels {
            let _ = writeln!(s, "lg_{}.{}:", name, l);
        }
        if !f.blocks[b as usize].labels.is_empty() && f.has_vla && f.dyn_stack {
            s.push_str("\tmov sp, x29\n");
        }
        for inst in &f.blocks[b as usize].insts {
            emit_inst(s, ast, f, inst);
        }
        let next = order.get(i + 1).copied();
        emit_term(s, f, &name, &f.blocks[b as usize].term, next, &mut tables);
    }
    let _ = writeln!(s, "\t.size {}, .-{}", name, name);
    // The jump tables this function's switches read. `.rodata`, position
    // independent: each entry is the SIGNED 32-bit distance from the table to
    // its block, so the sequence needs no relocation at run time and the table
    // survives `-fpic` unchanged.
    for (label, blocks) in tables {
        let _ = writeln!(s, "\t.section .rodata\n\t.p2align 2\n{}:", label);
        for b in blocks {
            let _ = writeln!(s, "\t.word .L{}_{} - {}", name, b, label);
        }
        s.push_str(&text_section(&name));
    }
}

/// The register every static stack object is addressed from: sp in an ordinary
/// function (`-fomit-frame-pointer` by construction), x29 when a `StackAlloc`
/// has made sp move inside the body.
fn frame_base(f: &MFunc) -> &'static str {
    if f.dyn_stack { "x29" } else { "sp" }
}

/// `sub sp, sp, #n` — the immediate is limited to imm12(<<12), so a very large
/// frame needs the constant in a scratch register first.
fn adjust_sp(s: &mut String, delta: i64) {
    let (op, n) = if delta < 0 {
        ("sub", -delta)
    } else {
        ("add", delta)
    };
    match isa::add_imm(n) {
        Some((v, 0)) => {
            let _ = writeln!(s, "\t{} sp, sp, #{}", op, v);
        }
        Some((v, sh)) => {
            let _ = writeln!(s, "\t{} sp, sp, #{}, lsl #{}", op, v, sh);
        }
        None => {
            mov_imm(s, isa::SCRATCH_GPR, n, true);
            let _ = writeln!(s, "\t{} sp, sp, x{}", op, isa::SCRATCH_GPR.num);
        }
    }
}

/// `add dst, base, #n` when the immediate fits, otherwise the two-add or
/// `movz/movk`+`add` form. The count is what `isa::add_imm`/`isa::mov_chain`
/// predict, which is why `SlotAddr` stays a computable cost (MECHANISM.md §G10).
fn add_imm_to(s: &mut String, dst: String, base: &str, n: i64) {
    match isa::add_imm(n) {
        Some((v, 0)) => {
            let _ = writeln!(s, "\tadd {}, {}, #{}", dst, base, v);
        }
        Some((v, sh)) => {
            let _ = writeln!(s, "\tadd {}, {}, #{}, lsl #{}", dst, base, v, sh);
        }
        None if n > 0 && n < (1 << 24) => {
            let (hi, lo) = (n >> 12, n & 0xfff);
            let _ = writeln!(s, "\tadd {}, {}, #{}, lsl #12", dst, base, hi);
            let _ = writeln!(s, "\tadd {}, {}, #{}", dst, dst, lo);
        }
        None => {
            mov_imm(s, isa::SCRATCH_GPR, n, true);
            let _ = writeln!(s, "\tadd {}, {}, x{}", dst, base, isa::SCRATCH_GPR.num);
        }
    }
}

fn mov_imm(s: &mut String, p: PReg, v: i64, is64: bool) {
    let r = isa::gpr_name(p.num, is64);
    for (kind, imm, shift) in isa::mov_chain(v, is64) {
        let m = match kind {
            MovKind::Z => "movz",
            MovKind::N => "movn",
            MovKind::K => "movk",
        };
        if shift == 0 {
            let _ = writeln!(s, "\t{} {}, #{}", m, r, imm);
        } else {
            let _ = writeln!(s, "\t{} {}, #{}, lsl #{}", m, r, imm, shift);
        }
    }
}

// ── registers and operands ─────────────────────────────────────────────────
fn reg(r: Reg, w: Width) -> String {
    match r {
        Reg::P(p) => isa::reg_name(p, w),
        // Only a bug can leave a virtual register here; make it loud rather than
        // emit something the assembler would accept.
        Reg::V(v) => format!("<v{}>", v),
    }
}

fn rhs(r: &Rhs, w: Width) -> String {
    match r {
        Rhs::Reg(x) => reg(*x, w),
        Rhs::Imm(k) => format!("#{}", k),
        Rhs::Shifted(x, k, n) => format!(
            "{}, {} #{}",
            reg(*x, w),
            match k {
                ShiftKind::Lsl => "lsl",
                ShiftKind::Lsr => "lsr",
                ShiftKind::Asr => "asr",
                ShiftKind::Ror => "ror",
            },
            n
        ),
        Rhs::Extended(x, e, n) => {
            let name = ext_name(*e);
            // an extended operand always reads the w-form for the b/h/w widths
            let sw = if matches!(
                e,
                ExtKind::Uxtx | ExtKind::Sxtx
            ) {
                Width::W64
            } else {
                Width::W32
            };
            if *n == 0 {
                format!("{}, {}", reg(*x, sw), name)
            } else {
                format!("{}, {} #{}", reg(*x, sw), name, n)
            }
        }
    }
}

fn ext_name(e: ExtKind) -> &'static str {
    match e {
        ExtKind::Uxtb => "uxtb",
        ExtKind::Uxth => "uxth",
        ExtKind::Uxtw => "uxtw",
        ExtKind::Uxtx => "uxtx",
        ExtKind::Sxtb => "sxtb",
        ExtKind::Sxth => "sxth",
        ExtKind::Sxtw => "sxtw",
        ExtKind::Sxtx => "sxtx",
    }
}

fn sym_text(ast: &Ast, s: &crate::hir::Sym, fname: &str) -> String {
    use crate::hir::Sym;
    match s {
        Sym::Global(i) | Sym::Tls(i) => sym_name(&ast.globals[*i as usize].name),
        Sym::Str(i) => format!(".LC{}", i),
        Sym::Func(n) => sym_name(n),
        Sym::Label(b) => format!(".L{}_{}", fname, b),
    }
}

fn addr(ast: &Ast, f: &MFunc, m: &AddrMode) -> String {
    match m {
        AddrMode::BaseImm { base, off } => {
            if *off == 0 {
                format!("[{}]", reg(*base, Width::W64))
            } else {
                format!("[{}, #{}]", reg(*base, Width::W64), off)
            }
        }
        AddrMode::BaseReg {
            base,
            idx,
            ext,
            shift,
        } => {
            let iw = match ext {
                Some(ExtKind::Uxtw) | Some(ExtKind::Sxtw) => Width::W32,
                _ => Width::W64,
            };
            let e = match ext {
                Some(e) => format!(", {}", ext_name(*e)),
                None => String::new(),
            };
            let sh = if *shift == 0 {
                String::new()
            } else if ext.is_none() {
                format!(", lsl #{}", shift)
            } else {
                format!(" #{}", shift)
            };
            format!(
                "[{}, {}{}{}]",
                reg(*base, Width::W64),
                reg(*idx, iw),
                e,
                sh
            )
        }
        AddrMode::PreIdx { base, off, .. } => {
            format!("[{}, #{}]!", reg(*base, Width::W64), off)
        }
        AddrMode::PostIdx { base, off, .. } => {
            format!("[{}], #{}", reg(*base, Width::W64), off)
        }
        AddrMode::Slot { slot, off } => {
            let (b, o) = (frame_base(f), f.slots[*slot as usize].off + off);
            if o == 0 {
                format!("[{}]", b)
            } else {
                format!("[{}, #{}]", b, o)
            }
        }
        // AAPCS64 §6.4: the outgoing-argument area begins at sp when the `bl`
        // executes, so it is named from sp even in a frame-pointer function.
        AddrMode::SpArg { off } => {
            if *off == 0 {
                "[sp]".to_string()
            } else {
                format!("[sp, #{}]", off)
            }
        }
        AddrMode::SymLo12 { base, sym } => {
            format!("[{}, #:lo12:{}]", reg(*base, Width::W64), sym_text(ast, sym, &f.name))
        }
        // The folded frame adjust (MECHANISM.md Part F R4.15). `delta < 0` is the prologue
        // pre-index `stp …, [sp, #-N]!` (allocate, then store at the new sp);
        // `delta > 0` the epilogue post-index `ldp …, [sp], #N` (load, then free).
        // The slot rides at offset 0, so [sp] IS its address after the writeback.
        AddrMode::FrameWb { delta, .. } => {
            if *delta < 0 {
                format!("[sp, #{}]!", delta)
            } else {
                format!("[sp], #{}", delta)
            }
        }
    }
}

fn mem_mnemonic(op: MemOp, load: bool) -> &'static str {
    match (op, load) {
        (MemOp::B, true) => "ldrb",
        (MemOp::SB, true) | (MemOp::SBX, true) => "ldrsb",
        (MemOp::H, true) => "ldrh",
        (MemOp::SH, true) | (MemOp::SHX, true) => "ldrsh",
        (MemOp::SW, true) => "ldrsw",
        (MemOp::W, true) | (MemOp::X, true) | (MemOp::S, true) | (MemOp::D, true)
        | (MemOp::Q, true) => "ldr",
        (MemOp::B, false) | (MemOp::SB, false) | (MemOp::SBX, false) => "strb",
        (MemOp::H, false) | (MemOp::SH, false) | (MemOp::SHX, false) => "strh",
        (_, false) => "str",
    }
}

/// The register form a load/store names: a narrow integer access uses the
/// w-form, and each sign-extending load names the width of its RESULT — which
/// the opcode carries, because the destination register does not (see
/// `MemOp::SB`).
fn mem_width(op: MemOp, _r: Reg, _f: &MFunc) -> Width {
    match op {
        MemOp::X | MemOp::SBX | MemOp::SHX | MemOp::SW => Width::W64,
        MemOp::S => Width::S,
        MemOp::D => Width::D,
        MemOp::Q => Width::Q,
        _ => Width::W32,
    }
}

/// EXT(gcc): substitute `%n` (with an optional width letter) by the register the
/// operand was pinned to. `%%` is a literal percent; a `"m"` operand prints as
/// `[xN]`, which is the whole of the memory-constraint contract zcc supports.
fn asm_text(tmpl: &str, ops: &[AsmSlot]) -> String {
    let mut out = String::with_capacity(tmpl.len());
    let b: Vec<char> = tmpl.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] != '%' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= b.len() {
            break;
        }
        if b[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let modifier = if b[i].is_ascii_digit() { None } else { Some(b[i]) };
        if modifier.is_some() {
            i += 1;
        }
        let mut n = 0usize;
        let mut any = false;
        while i < b.len() && b[i].is_ascii_digit() {
            n = n * 10 + b[i] as usize - '0' as usize;
            i += 1;
            any = true;
        }
        if !any || n >= ops.len() {
            out.push('%');
            if let Some(m) = modifier {
                out.push(m);
            }
            continue;
        }
        let o = ops[n];
        if o.mem {
            out.push_str(&format!("[{}]", isa::reg_name(o.reg, Width::W64)));
            continue;
        }
        let w = match modifier {
            Some('w') => Width::W32,
            Some('x') => Width::W64,
            Some('s') => Width::S,
            Some('d') => Width::D,
            Some('q') => Width::Q,
            _ => o.w,
        };
        out.push_str(&isa::reg_name(o.reg, w));
    }
    out
}

/// The `Vn` spelling of a v register, for the vector aliases (`mov Vd.16b, Vn.16b`).
fn vec_name(r: Reg) -> String {
    match r {
        Reg::P(p) => format!("v{}", p.num),
        Reg::V(v) => format!("<v{}>", v),
    }
}

fn cc_name(c: CC) -> &'static str {
    match c {
        CC::Eq => "eq",
        CC::Ne => "ne",
        CC::Hs => "hs",
        CC::Lo => "lo",
        CC::Mi => "mi",
        CC::Pl => "pl",
        CC::Vs => "vs",
        CC::Vc => "vc",
        CC::Hi => "hi",
        CC::Ls => "ls",
        CC::Ge => "ge",
        CC::Lt => "lt",
        CC::Gt => "gt",
        CC::Le => "le",
    }
}

fn emit_inst(s: &mut String, ast: &Ast, f: &MFunc, i: &MInst) {
    match i {
        MInst::Alu {
            op,
            w,
            dst,
            a,
            b,
            flags,
        } => {
            let m = match (op, flags.is_some()) {
                (AluOp::Add, false) => "add",
                (AluOp::Add, true) => "adds",
                (AluOp::Sub, false) => "sub",
                (AluOp::Sub, true) => "subs",
                (AluOp::And, false) => "and",
                (AluOp::And, true) => "ands",
                (AluOp::Orr, _) => "orr",
                (AluOp::Eor, _) => "eor",
                (AluOp::Bic, _) => "bic",
                (AluOp::Orn, _) => "orn",
                (AluOp::Eon, _) => "eon",
                (AluOp::Lsl, _) => "lsl",
                (AluOp::Lsr, _) => "lsr",
                (AluOp::Asr, _) => "asr",
                (AluOp::Mul, _) => "mul",
                (AluOp::SDiv, _) => "sdiv",
                (AluOp::UDiv, _) => "udiv",
                (AluOp::SMulH, _) => "smulh",
                (AluOp::UMulH, _) => "umulh",
            };
            let _ = writeln!(
                s,
                "\t{} {}, {}, {}",
                m,
                reg(*dst, *w),
                reg(*a, *w),
                rhs(b, *w)
            );
        }
        MInst::Alu3 { op, w, dst, a, b, c } => {
            let m = match op {
                Alu3Op::Madd => "madd",
                Alu3Op::Msub => "msub",
            };
            let _ = writeln!(
                s,
                "\t{} {}, {}, {}, {}",
                m,
                reg(*dst, *w),
                reg(*a, *w),
                reg(*b, *w),
                reg(*c, *w)
            );
        }
        MInst::Cmp { kind, w, a, b, .. } => {
            let m = match kind {
                CmpKind::Cmp => "cmp",
                CmpKind::Cmn => "cmn",
                CmpKind::Tst => "tst",
            };
            let _ = writeln!(s, "\t{} {}, {}", m, reg(*a, *w), rhs(b, *w));
        }
        MInst::MovImm { w, dst, imm } => match dst {
            Reg::P(p) => mov_imm(s, *p, *imm, w.is64()),
            Reg::V(v) => {
                let _ = writeln!(s, "\t<mov v{}, #{}>", v, imm);
            }
        },
        MInst::FMovImm { w, dst, bits } => {
            // The assembler takes the VALUE, not the encoding; printing it with
            // enough digits to round-trip is what makes the two agree.
            let v = match w {
                Width::S => f32::from_bits(*bits as u32) as f64,
                _ => f64::from_bits(*bits),
            };
            let _ = writeln!(s, "\tfmov {}, #{:?}", reg(*dst, *w), v);
        }
        MInst::Ext { op, w, dst, src } => {
            let m = match op {
                ExtOp::Sxtb => "sxtb",
                ExtOp::Sxth => "sxth",
                ExtOp::Sxtw => "sxtw",
                ExtOp::Uxtb => "uxtb",
                ExtOp::Uxth => "uxth",
                // the `w`-form move IS the zero-extension
                ExtOp::Uxtw => "mov",
            };
            // The source of every extend is named in its w-form. The
            // DESTINATION follows the instruction: `sxtw` writes an x-register
            // and says so, while the 32-bit zero-extension IS a `w`-form move —
            // `mov w0, w0` — and naming an x-register there would not assemble.
            let dw = match op {
                ExtOp::Uxtw => Width::W32,
                _ => *w,
            };
            let _ = writeln!(s, "\t{} {}, {}", m, reg(*dst, dw), reg(*src, Width::W32));
        }
        MInst::Load { op, dst, mem, .. } => {
            let _ = writeln!(
                s,
                "\t{} {}, {}",
                mem_mnemonic(*op, true),
                reg(*dst, mem_width(*op, *dst, f)),
                addr(ast, f, mem)
            );
        }
        MInst::Store { op, src, mem, .. } => {
            let _ = writeln!(
                s,
                "\t{} {}, {}",
                mem_mnemonic(*op, false),
                reg(*src, mem_width(*op, *src, f)),
                addr(ast, f, mem)
            );
        }
        MInst::Bfx { signed, w, dst, src, lsb, width } => {
            let _ = writeln!(
                s,
                "\t{} {}, {}, #{}, #{}",
                if *signed { "sbfx" } else { "ubfx" },
                reg(*dst, *w),
                reg(*src, *w),
                lsb,
                width
            );
        }
        MInst::Pair { w, load, a, b, mem } => {
            let _ = writeln!(
                s,
                "\t{} {}, {}, {}",
                if *load { "ldp" } else { "stp" },
                reg(*a, *w),
                reg(*b, *w),
                addr(ast, f, mem)
            );
        }
        MInst::Adrp { dst, sym, got } => {
            let n = sym_text(ast, sym, &f.name);
            let _ = if *got {
                writeln!(s, "\tadrp {}, :got:{}", reg(*dst, Width::W64), n)
            } else {
                writeln!(s, "\tadrp {}, {}", reg(*dst, Width::W64), n)
            };
        }
        MInst::AddLo12 {
            dst,
            base,
            sym,
            got,
        } => {
            let n = sym_text(ast, sym, &f.name);
            let _ = if *got {
                writeln!(
                    s,
                    "\tldr {}, [{}, #:got_lo12:{}]",
                    reg(*dst, Width::W64),
                    reg(*base, Width::W64),
                    n
                )
            } else {
                writeln!(
                    s,
                    "\tadd {}, {}, #:lo12:{}",
                    reg(*dst, Width::W64),
                    reg(*base, Width::W64),
                    n
                )
            };
        }
        MInst::CSel {
            op,
            w,
            dst,
            a,
            b,
            cc,
            ..
        } => {
            let m = match op {
                CSelOp::Csel => "csel",
                CSelOp::Csinc => "csinc",
                CSelOp::Csinv => "csinv",
                CSelOp::Csneg => "csneg",
            };
            let _ = writeln!(
                s,
                "\t{} {}, {}, {}, {}",
                m,
                reg(*dst, *w),
                reg(*a, *w),
                reg(*b, *w),
                cc_name(*cc)
            );
        }
        MInst::CSet { w, dst, cc, .. } => {
            let _ = writeln!(s, "\tcset {}, {}", reg(*dst, *w), cc_name(*cc));
        }
        MInst::VAlu { op, arr, dst, a, b } => {
            let m = match op {
                FpOp::Fadd => "fadd",
                FpOp::Fsub => "fsub",
                FpOp::Fmul => "fmul",
                FpOp::Fdiv => "fdiv",
            };
            // the vector spelling of the same register the scalar forms print
            // as `d`/`s`/`q`: `v<n>.<arrangement>`
            let v = |r: Reg| match r {
                Reg::P(p) => format!("v{}.{}", p.num, arr.suffix()),
                Reg::V(x) => format!("<v{}>", x),
            };
            let _ = writeln!(s, "\t{} {}, {}, {}", m, v(*dst), v(*a), v(*b));
        }
        MInst::FpAlu { op, w, dst, a, b } => {
            let m = match op {
                FpOp::Fadd => "fadd",
                FpOp::Fsub => "fsub",
                FpOp::Fmul => "fmul",
                FpOp::Fdiv => "fdiv",
            };
            let _ = writeln!(
                s,
                "\t{} {}, {}, {}",
                m,
                reg(*dst, *w),
                reg(*a, *w),
                reg(*b, *w)
            );
        }
        MInst::FpUn { op, w, dst, src, sw } => {
            let m = match op {
                FpUnOp::Fneg => "fneg",
                FpUnOp::Fabs => "fabs",
                FpUnOp::Fsqrt => "fsqrt",
                FpUnOp::Fcvt => "fcvt",
            };
            let _ = writeln!(s, "\t{} {}, {}", m, reg(*dst, *w), reg(*src, *sw));
        }
        MInst::FpCmp { w, a, b, zero, .. } => {
            let _ = if *zero {
                writeln!(s, "\tfcmp {}, #0.0", reg(*a, *w))
            } else {
                writeln!(s, "\tfcmp {}, {}", reg(*a, *w), reg(*b, *w))
            };
        }
        MInst::FpCvt {
            op,
            dw,
            sw,
            dst,
            src,
        } => {
            let m = match op {
                CvtOp::Scvtf => "scvtf",
                CvtOp::Ucvtf => "ucvtf",
                CvtOp::Fcvtzs => "fcvtzs",
                CvtOp::Fcvtzu => "fcvtzu",
            };
            let _ = writeln!(s, "\t{} {}, {}", m, reg(*dst, *dw), reg(*src, *sw));
        }
        MInst::FMov { dw, sw, dst, src } => {
            // DDI 0487 C7: `fmov` has no 128-bit form — a whole v register moves
            // as a 16-byte vector (`mov Vd.16b, Vn.16b`, the ORR alias).
            if *dw == Width::Q || *sw == Width::Q {
                let _ = writeln!(s, "\tmov {}.16b, {}.16b", vec_name(*dst), vec_name(*src));
            } else {
                let _ = writeln!(s, "\tfmov {}, {}", reg(*dst, *dw), reg(*src, *sw));
            }
        }
        MInst::Call { callee, tail, .. } => {
            let m = if *tail { "b" } else { "bl" };
            match callee {
                CallTarget::Direct(n) => {
                    let _ = writeln!(s, "\t{} {}", m, sym_name(n));
                }
                CallTarget::Indirect(r) => {
                    let _ = writeln!(
                        s,
                        "\t{} {}",
                        if *tail { "br" } else { "blr" },
                        reg(*r, Width::W64)
                    );
                }
            }
        }
        MInst::Copy { w, dst, src } if *w == Width::Q => {
            let _ = writeln!(s, "\tmov {}.16b, {}.16b", vec_name(*dst), vec_name(*src));
        }
        MInst::Copy { w, dst, src } => {
            let _ = writeln!(s, "\tmov {}, {}", reg(*dst, *w), reg(*src, *w));
        }
        MInst::ParallelCopy(_) => {
            let _ = writeln!(s, "\t<parallel copy not sequentialized>");
        }
        MInst::Spill { slot, src, w } => {
            let o = f.slots[*slot as usize].off;
            let _ = writeln!(s, "\tstr {}, [{}, #{}]", reg(*src, *w), frame_base(f), o);
        }
        MInst::Reload { slot, dst, w } => {
            let o = f.slots[*slot as usize].off;
            let _ = writeln!(s, "\tldr {}, [{}, #{}]", reg(*dst, *w), frame_base(f), o);
        }
        MInst::SlotAddr { dst, slot, off } => {
            let o = f.slots[*slot as usize].off + off;
            add_imm_to(s, reg(*dst, Width::W64), frame_base(f), o as i64);
        }
        MInst::SpAddr { dst, off } => {
            add_imm_to(s, reg(*dst, Width::W64), "sp", *off as i64);
        }
        // The frame adjust, now an ordinary instruction (MECHANISM.md Part F R4.15) rather
        // than something `emit` invents from `frame_size`. `adjust_sp` prints the
        // `sub`/`add` (and, for a frame past imm12, the scratch-register form whose
        // length the cost model already predicts).
        MInst::SpAdj { delta } => adjust_sp(s, *delta as i64),
        MInst::LdAxr { w, dst, addr } => {
            let _ = writeln!(
                s,
                "\tldaxr {}, [{}]",
                reg(*dst, *w),
                reg(*addr, Width::W64)
            );
        }
        MInst::StlXr {
            w,
            status,
            src,
            addr,
        } => {
            let _ = writeln!(
                s,
                "\tstlxr {}, {}, [{}]",
                reg(*status, Width::W32),
                reg(*src, *w),
                reg(*addr, Width::W64)
            );
        }
        MInst::Stlr { w, src, addr } => {
            let _ = writeln!(
                s,
                "\tstlr {}, [{}]",
                reg(*src, *w),
                reg(*addr, Width::W64)
            );
        }
        MInst::Dmb => s.push_str("\tdmb ish\n"),
        MInst::Mrs { dst } => {
            let _ = writeln!(s, "\tmrs {}, tpidr_el0", reg(*dst, Width::W64));
        }
        MInst::AddTprel { dst, base, sym, hi } => {
            let n = sym_text(ast, sym, &f.name);
            let _ = if *hi {
                writeln!(
                    s,
                    "\tadd {}, {}, #:tprel_hi12:{}, lsl #12",
                    reg(*dst, Width::W64),
                    reg(*base, Width::W64),
                    n
                )
            } else {
                writeln!(
                    s,
                    "\tadd {}, {}, #:tprel_lo12_nc:{}",
                    reg(*dst, Width::W64),
                    reg(*base, Width::W64),
                    n
                )
            };
        }
        MInst::Asm { tmpl, ops } => {
            s.push('\t');
            s.push_str(&asm_text(tmpl, ops));
            s.push('\n');
        }
        // AAPCS64 §6.2.2: sp must stay 16-byte aligned, so the byte count is
        // rounded up before it is subtracted. The new object starts ABOVE the
        // outgoing-argument area, which stays pinned at sp for the next call.
        MInst::StackAlloc { dst, size } => {
            let (ip, d) = (isa::SCRATCH_GPR.num, reg(*dst, Width::W64));
            let _ = writeln!(
                s,
                "\tadd x{ip}, {}, #15\n\tand x{ip}, x{ip}, #0xfffffffffffffff0\n\tsub sp, sp, x{ip}",
                reg(*size, Width::W64)
            );
            if f.outgoing == 0 {
                let _ = writeln!(s, "\tmov {}, sp", d);
            } else {
                let _ = writeln!(s, "\tadd {}, sp, #{}", d, f.outgoing);
            }
        }
    }
}

fn emit_term(
    s: &mut String,
    f: &MFunc,
    name: &str,
    t: &MTerm,
    next: Option<MBlockId>,
    tables: &mut Vec<(String, Vec<MBlockId>)>,
) {
    let lbl = |b: MBlockId| format!(".L{}_{}", name, b);
    let jump = |s: &mut String, b: MBlockId| {
        if Some(b) != next {
            let _ = writeln!(s, "\tb {}", lbl(b));
        }
    };
    match t {
        MTerm::B(x) => jump(s, x.block),
        MTerm::Bcc(cc, _, x, y) => {
            let _ = writeln!(s, "\tb.{} {}", cc_name(*cc), lbl(x.block));
            jump(s, y.block);
        }
        MTerm::Cbz { w, reg: r, zero, t: x, f: y } => {
            let _ = writeln!(
                s,
                "\t{} {}, {}",
                if *zero { "cbz" } else { "cbnz" },
                reg(*r, *w),
                lbl(x.block)
            );
            jump(s, y.block);
        }
        MTerm::Tb {
            w,
            reg: r,
            bit,
            set,
            t: x,
            f: y,
        } => {
            let _ = writeln!(
                s,
                "\t{} {}, #{}, {}",
                if *set { "tbnz" } else { "tbz" },
                reg(*r, *w),
                bit,
                lbl(x.block)
            );
            jump(s, y.block);
        }
        // A dense switch: `adrp`/`add` the table, read the entry for the index,
        // and branch to `table + entry`. IP0/IP1 (x16/x17) are reserved exactly
        // so a sequence like this needs no allocated register (MECHANISM.md §G5.1), and
        // a terminator is the one place both are certainly free.
        MTerm::Switch { idx, table, .. } => {
            let label = format!(".Ljt_{}_{}", name, tables.len());
            let _ = writeln!(
                s,
                "\tadrp x16, {0}\n\tadd x16, x16, :lo12:{0}\n\tldrsw x17, [x16, {1}, lsl #2]\n\
                 \tadd x16, x16, x17\n\tbr x16",
                label,
                reg(*idx, Width::W64)
            );
            tables.push((label, table.iter().map(|t| t.block).collect()));
        }
        MTerm::Ret => {
            if f.dyn_stack {
                // reclaim everything a `StackAlloc` took, then the caller's x29
                let _ = writeln!(
                    s,
                    "\tmov sp, x29\n\tldr x29, [sp, #{}]",
                    f.slots[f.fp_slot as usize].off
                );
            }
            // R4.15: an ordinary frame's `add sp` is a real `SpAdj` (or folded
            // into the last restore pair) before this `Ret`; only a dynamic frame
            // still prints its adjust here.
            if f.frame_size > 0 && f.dyn_stack {
                adjust_sp(s, f.frame_size as i64);
            }
            s.push_str("\tret\n");
        }
        MTerm::BrReg(r, _) => {
            let _ = writeln!(s, "\tbr {}", reg(*r, Width::W64));
        }
        // Falling off a non-void function is undefined (C99 6.9.1p12); trap
        // rather than run into the next function's prologue.
        MTerm::Unreachable => s.push_str("\tbrk #1\n"),
    }
}

// ── data ───────────────────────────────────────────────────────────────────
fn data(s: &mut String, ast: &Ast) {
    for (i, str_) in ast.strs.iter().enumerate() {
        s.push_str("\t.section .rodata\n\t.p2align 3\n");
        let _ = writeln!(s, ".LC{}:", i);
        // C99 6.4.5p6: a string literal's array includes the terminating null,
        // which the parser does NOT store in `strs` (the type carries the extra
        // byte). Every consumer must add it back.
        let mut b = str_.clone();
        b.push(0);
        bytes(s, &b);
    }
    for g in &ast.globals {
        if g.is_extern {
            continue; // a declaration reserves no storage
        }
        global(s, ast, g);
    }
}

fn global(s: &mut String, ast: &Ast, g: &Global) {
    let name = sym_name(&g.name);
    let size = ast.tt.size(g.ty).max(1);
    let align = ast.tt.data_align(g.ty).max(1);
    let p2 = align.trailing_zeros();
    let bss = matches!(g.init, GInit::None);
    if g.is_tls {
        s.push_str(if bss { "\t.section .tbss,\"awT\",@nobits\n" } else { "\t.section .tdata,\"awT\"\n" });
    } else {
        s.push_str(if bss { "\t.bss\n" } else { "\t.data\n" });
    }
    let _ = writeln!(s, "\t.p2align {}", p2);
    if g.is_weak {
        let _ = writeln!(s, "\t.weak {}", name);
    } else if !g.is_static {
        let _ = writeln!(s, "\t.globl {}", name);
    }
    let _ = writeln!(s, "\t.type {}, %object\n\t.size {}, {}\n{}:", name, name, size, name);
    if bss {
        let _ = writeln!(s, "\t.zero {}", size);
    } else {
        let mut at = 0u32;
        init(s, &g.init, size, &mut at);
        if at < size {
            let _ = writeln!(s, "\t.zero {}", size - at);
        }
    }
}

/// Emit an initializer, tracking how many bytes have been written so gaps
/// (padding between struct members, an incompletely initialized array) are
/// filled with zeros.
fn init(s: &mut String, g: &GInit, size: u32, at: &mut u32) {
    match g {
        GInit::None => {}
        GInit::Num(k) => {
            let d = match size {
                1 => ".byte",
                2 => ".hword",
                4 => ".word",
                _ => ".xword",
            };
            let _ = writeln!(s, "\t{} {}", d, k);
            *at += size;
        }
        GInit::Str(i) => {
            let _ = writeln!(s, "\t.xword .LC{}", i);
            *at += 8;
        }
        GInit::StrOff(i, off) => {
            let _ = writeln!(s, "\t.xword .LC{}+{}", i, off);
            *at += 8;
        }
        GInit::Addr(n, off) => {
            let _ = writeln!(s, "\t.xword {}+{}", sym_name(n), off);
            *at += 8;
        }
        // EXT(gcc): &&a - &&b, the static jump table idiom
        GInit::Diff(a, b) => {
            let _ = writeln!(s, "\t.word {}-{}", sym_name(a), sym_name(b));
            *at += 4;
        }
        GInit::Bytes(b) => {
            bytes(s, b);
            *at += b.len() as u32;
        }
        GInit::List(items) => {
            for (off, isz, it) in items {
                if *off > *at {
                    let _ = writeln!(s, "\t.zero {}", off - *at);
                    *at = *off;
                }
                init(s, it, *isz, at);
            }
        }
    }
}

fn bytes(s: &mut String, b: &[u8]) {
    if b.is_empty() {
        return;
    }
    let mut line = String::from("\t.byte ");
    let mut first = true;
    for x in b {
        if !first {
            line.push(',');
        }
        let _ = write!(line, "{}", x);
        first = false;
        // Wrap purely for readability of the .s; the assembled bytes are
        // identical at any width, and the width is fixed so emission stays
        // deterministic (tests/determinism.sh).
        if line.len() > 100 {
            let _ = writeln!(s, "{}", line);
            line = String::from("\t.byte ");
            first = true;
        }
    }
    if !first {
        let _ = writeln!(s, "{}", line);
    }
}
