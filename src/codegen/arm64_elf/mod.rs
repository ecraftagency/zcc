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
use crate::ast::{Ast, Ty, TypeId, VOID};
use crate::ir::{self, Inst, Term, Tmp};
use crate::opt::AbiHome;
use std::fmt::Write;

// Submodules (Law-1 seams): encoding = Side-II spec tables; lower/emit/peephole = Side-I
// algorithm (value-contract, per-Inst emitter, cost-square passes). emit.rs holds inherent
// Cg methods (resolved by type — no glob needed); its parent (this module) owns the Cg
// struct, so all children see its private fields.
mod emit;
mod encoding;
mod lower;
mod peephole;
use encoding::*;
use lower::{emit_module_tail, emit_params};
use peephole::*;


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
                // (loops.rs::hoist_loop_consts); size impact ≈ 0 (hoists are size-neutral).
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
