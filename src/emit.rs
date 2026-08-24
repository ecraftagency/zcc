// MIR(final) → AArch64-ELF assembly text (REARCH.md §9; THEORY II-4 for the
// ELF/relocation side).
//
// This file makes NO decisions. Every instruction was already chosen, every
// register already assigned, every offset already computed; printing is a
// one-to-one function from `MInst` to a line of text. That is the property the
// cost model depends on (REARCH §10): `cost(f) = |MIR_final(f)|` needs no
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
    s
}

/// The parser marks a name whose spelling is final with a \x01 prefix. On ELF
/// no name gets a leading underscore anyway, so the prefix is simply stripped.
fn sym_name(n: &str) -> String {
    n.strip_prefix('\u{1}').unwrap_or(n).to_string()
}

// ── functions ──────────────────────────────────────────────────────────────
fn func(s: &mut String, ast: &Ast, f: &MFunc) {
    let name = sym_name(&f.name);
    s.push_str("\t.text\n\t.p2align 2\n");
    if f.is_weak {
        let _ = writeln!(s, "\t.weak {}", name);
    } else if !f.is_static {
        let _ = writeln!(s, "\t.globl {}", name);
    }
    let _ = writeln!(s, "\t.type {}, %function\n{}:", name, name);
    // One frame adjustment, by construction (§8): nothing else moves sp.
    if f.frame_size > 0 {
        adjust_sp(s, -(f.frame_size as i64));
    }
    let order: Vec<MBlockId> = if f.order.is_empty() {
        (0..f.blocks.len() as MBlockId).collect()
    } else {
        f.order.clone()
    };
    for (i, &b) in order.iter().enumerate() {
        let _ = writeln!(s, ".L{}_{}:", name, b);
        for inst in &f.blocks[b as usize].insts {
            emit_inst(s, ast, f, inst);
        }
        let next = order.get(i + 1).copied();
        emit_term(s, f, &name, &f.blocks[b as usize].term, next);
    }
    let _ = writeln!(s, "\t.size {}, .-{}", name, name);
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
        Sym::Global(i) => sym_name(&ast.globals[*i as usize].name),
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
        // Every stack object is addressed from sp: there is no frame pointer
        // (`-fomit-frame-pointer` by construction).
        AddrMode::Slot { slot, off } => {
            let o = f.slots[*slot as usize].off + off;
            if o == 0 {
                "[sp]".to_string()
            } else {
                format!("[sp, #{}]", o)
            }
        }
        AddrMode::SymLo12 { base, sym } => {
            format!("[{}, #:lo12:{}]", reg(*base, Width::W64), sym_text(ast, sym, &f.name))
        }
    }
}

fn mem_mnemonic(op: MemOp, load: bool) -> &'static str {
    match (op, load) {
        (MemOp::B, true) => "ldrb",
        (MemOp::SB, true) => "ldrsb",
        (MemOp::H, true) => "ldrh",
        (MemOp::SH, true) => "ldrsh",
        (MemOp::SW, true) => "ldrsw",
        (MemOp::W, true) | (MemOp::X, true) | (MemOp::S, true) | (MemOp::D, true) => "ldr",
        (MemOp::B, false) | (MemOp::SB, false) => "strb",
        (MemOp::H, false) | (MemOp::SH, false) => "strh",
        (_, false) => "str",
    }
}

/// The register form a load/store names: a narrow integer access uses the
/// w-form, `ldrsb/ldrsh/ldrsw` name the width of the RESULT.
fn mem_width(op: MemOp, r: Reg, f: &MFunc) -> Width {
    match op {
        MemOp::X => Width::W64,
        MemOp::S => Width::S,
        MemOp::D => Width::D,
        MemOp::SB | MemOp::SH | MemOp::SW => match r {
            Reg::V(v) => f.vregs[v as usize].width,
            Reg::P(_) => Width::W64,
        },
        _ => Width::W32,
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
        MInst::Ext { op, w, dst, src } => {
            let m = match op {
                ExtOp::Sxtb => "sxtb",
                ExtOp::Sxth => "sxth",
                ExtOp::Sxtw => "sxtw",
                ExtOp::Uxtb => "uxtb",
                ExtOp::Uxth => "uxth",
            };
            // the source of every extend is named in its w-form
            let _ = writeln!(s, "\t{} {}, {}", m, reg(*dst, *w), reg(*src, Width::W32));
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
            let _ = writeln!(s, "\tfmov {}, {}", reg(*dst, *dw), reg(*src, *sw));
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
        MInst::Copy { w, dst, src } => {
            let _ = writeln!(s, "\tmov {}, {}", reg(*dst, *w), reg(*src, *w));
        }
        MInst::ParallelCopy(_) => {
            let _ = writeln!(s, "\t<parallel copy not sequentialized>");
        }
        MInst::Spill { slot, src, w } => {
            let o = f.slots[*slot as usize].off;
            let _ = writeln!(s, "\tstr {}, [sp, #{}]", reg(*src, *w), o);
        }
        MInst::Reload { slot, dst, w } => {
            let o = f.slots[*slot as usize].off;
            let _ = writeln!(s, "\tldr {}, [sp, #{}]", reg(*dst, *w), o);
        }
        MInst::SlotAddr { dst, slot, off } => {
            let o = f.slots[*slot as usize].off + off;
            let _ = writeln!(s, "\tadd {}, sp, #{}", reg(*dst, Width::W64), o);
        }
    }
}

fn emit_term(s: &mut String, f: &MFunc, name: &str, t: &MTerm, next: Option<MBlockId>) {
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
        MTerm::Switch { .. } => {
            // R3.3 lowers a dense switch to adr+ldr+br; until then isel never
            // builds this terminator (it emits a compare chain instead).
            let _ = writeln!(s, "\t<switch table not lowered>");
        }
        MTerm::Ret => {
            if f.frame_size > 0 {
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
