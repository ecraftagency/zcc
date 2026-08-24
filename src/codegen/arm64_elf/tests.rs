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
    let out = peephole_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x24"), 0, "the redundant reload must be dropped");
    assert_eq!(count(&out, "mov x24, x0"), 1, "the store to the home must be kept");
    assert!(out.contains("add x0, x0, x1"), "the real op is untouched");
}

// A DEF between the two movs BREAKS the equivalence — the reload is NOT redundant and
// must be KEPT (x0 was clobbered by the mul).
#[test]
fn peephole_keeps_move_after_clobber() {
    let body = "\tmov x24, x0\n\tmul x0, x5, x6\n\tmov x0, x24\n";
    let out = peephole_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x24"), 1, "x0 was clobbered ⟹ the reload is real");
}

// Jump-threading: a chain of empty `b`-only forwarders collapses; every branch to the
// chain retargets to the final real block, and the dead forwarder blocks are removed.
#[test]
fn thread_collapses_forwarder_chain() {
    let body = "\tcbz x0, .La\n\tret\n.La:\n\tb .Lb\n.Lb:\n\tb .Lc\n.Lc:\n\tmov x0, #1\n\tret\n";
    let out = super::thread_asm_branches(body);
    assert!(out.contains("cbz x0, .Lc"), "branch retargeted through the chain to the final block");
    assert_eq!(count(&out, ".La:"), 0, "dead forwarder .La deleted");
    assert_eq!(count(&out, ".Lb:"), 0, "dead forwarder .Lb deleted");
    assert!(out.contains(".Lc:"), "the real target block is kept");
    assert!(out.contains("mov x0, #1"), "real code untouched");
}

// A genuine empty self-loop (`for(;;);` → `L: b L`) is NOT a forwarder: it must survive.
#[test]
fn thread_preserves_self_loop() {
    let body = "\tret\n.Lloop:\n\tb .Lloop\n";
    let out = super::thread_asm_branches(body);
    assert!(out.contains(".Lloop:") && out.contains("b .Lloop"), "the infinite loop is intact");
}

// A forwarder reached by FALL-THROUGH cannot be deleted (nothing could replace the
// fall-through edge without adding a branch) — retargeted-through but block kept.
#[test]
fn thread_keeps_fallthrough_forwarder() {
    let body = "\tadd x0, x0, x1\n.Lf:\n\tb .Lg\n.Lg:\n\tret\n";
    let out = super::thread_asm_branches(body);
    assert!(out.contains(".Lf:"), "fall-through forwarder must be kept");
}

// REGRESSION (981019-1): a forwarder reached by fall-through THROUGH an intervening empty
// BUT branch-targeted label must be kept. `.Le` (referenced by `b .Le`) is empty and falls
// into `.Lf: b .Lx` — deleting .Lf would fall .Le's arrivals into the next block (a bug that
// rerouted a return path into `bl abort`). The referenced label resets fall-through-reach.
#[test]
fn thread_keeps_forwarder_after_referenced_empty_label() {
    let body = "\tcbz x0, .Le\n\tb .Lx\n\tbl abort\n\tb .Ly\n.Le:\n.Lf:\n\tb .Lx\n.Lz:\n\tbl abort\n.Lx:\n\tret\n";
    let out = super::thread_asm_branches(body);
    assert!(out.contains(".Lf:") && out.contains("b .Lx"), ".Lf must survive: .Le falls into it");
    assert!(out.contains(".Le:"), "the referenced empty label is kept");
}

// A body that forms a label ADDRESS (computed goto / jump table) is left untouched.
#[test]
fn thread_bails_on_computed_goto() {
    let body = "\tbr x0\n.Lx:\n\tb .Ly\n.Ly:\n\tret\n";
    assert_eq!(super::thread_asm_branches(body), body, "computed-goto body is not rewritten");
}

// A label (basic-block boundary) FLUSHES the model — a cross-boundary equivalence must
// never be assumed (the predecessor might not have set it).
#[test]
fn peephole_flushes_at_label() {
    let body = "\tmov x24, x0\n.Lx:\n\tmov x0, x24\n";
    let out = peephole_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x24"), 1, "must not elide across a label");
}

// An UNRECOGNIZED mnemonic flushes conservatively (safety over coverage).
#[test]
fn peephole_flushes_on_unknown() {
    let body = "\tmov x24, x0\n\tzzz x0, x1\n\tmov x0, x24\n";
    let out = peephole_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x24"), 1, "unknown insn ⟹ flush ⟹ keep the reload");
}

// Round-trip `mov x24,x0; mov x0,x24` is redundant and dropped; a genuinely distinct
// move (different value) is preserved.
#[test]
fn peephole_preserves_distinct_move() {
    let body = "\tmov x24, x0\n\tmov x0, x24\n\tmov x1, x25\n";
    let out = peephole_moves(body, 1);
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

use super::fmov_residency;

// Phase 4.2: a `fmov x,d` then `fmov d,x` round-trip where the reload target already holds the
// value (via the store) is redundant and dropped — the FP residency win.
#[test]
fn fmov_residency_drops_reload_after_store() {
    // d17 ← x10 (store), then x10 ← d17 (reload): x10 already ≡ d17 ⟹ the reload is dropped.
    let body = "\tfmov x10, d0\n\tfmov d17, x10\n\tfmov x10, d17\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "fmov x10, d17"), 0, "reload of a value x10 already holds ⟹ drop");
    assert_eq!(count(&out, "fmov d17, x10"), 1, "the store stays");
}

// A restore `fmov d0, x10` where d0 already equals x10 (nothing wrote d0 since) is dropped.
#[test]
fn fmov_residency_drops_redundant_restore() {
    let body = "\tfmov x10, d0\n\tfmov d17, x10\n\tfmov d0, x10\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "fmov d0, x10"), 0, "d0 still ≡ x10 ⟹ redundant restore dropped");
}

// TEETH: once the FP value is REDEFINED (fmul writes d0), a later `fmov d0,x10` is NOT
// redundant — d0 no longer equals x10. Mis-dropping it would lose the recomputed value.
#[test]
fn fmov_residency_keeps_after_redef() {
    let body = "\tfmov x10, d0\n\tfmul d0, d0, d1\n\tfmov d0, x10\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "fmov d0, x10"), 1, "d0 redefined by fmul ⟹ restore is live ⟹ keep");
}

// TEETH: a block boundary (branch) flushes the equivalence model — no cross-block drop.
#[test]
fn fmov_residency_flushes_at_branch() {
    let body = "\tfmov x10, d0\n\tfmov d17, x10\n\tb .L1\n\tfmov x10, d17\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "fmov x10, d17"), 1, "branch flushes ⟹ conservative keep");
}

// TEETH (pr93908): a post-index store `str Rt,[xB],#k` MODIFIES the base xB (xB += k). The
// saved `mov x1,x0` no longer equals x0 afterward, so the restore `mov x0,x1` is NOT redundant
// and must survive. Before the writeback-invalidation fix, residency saw str as NO_DEF (base
// unchanged), wrongly dropped the restore, and bar() returned base+8 instead of base.
#[test]
fn fmov_residency_invalidates_writeback_base() {
    let body = "\tmov x1, x0\n\tstr w10, [x0], #8\n\tmov x0, x1\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "mov x0, x1"), 1, "x0 changed by the post-index ⟹ restore is live");
}

// Pre-index `[xB, #k]!` equally writes back the base — same invalidation.
#[test]
fn fmov_residency_invalidates_preindex_base() {
    let body = "\tmov x1, x0\n\tstr w10, [x0, #8]!\n\tmov x0, x1\n";
    let out = fmov_residency(body);
    assert_eq!(count(&out, "mov x0, x1"), 1, "pre-index writeback changes x0 ⟹ keep restore");
}

use super::fold_fp_imm;

// Phase 4.1: the imm8-encodable double 3.0 (bits 0x4008..., built as mov#0/movk#0x4008<<48/fmov)
// collapses to one `fmov d1, #3.0`. xN is redefined (mov x11,#5) before any read ⟹ dead ⟹ safe.
#[test]
fn fp_imm_folds_encodable_double() {
    let body = "\tmov x11, #0\n\tmovk x11, #16392, lsl #48\n\tfmov d1, x11\n\tmov x11, #5\n";
    let out = fold_fp_imm(body);
    assert_eq!(count(&out, "fmov d1, #3.0"), 1, "3.0 is imm8-encodable ⟹ one fmov #imm");
    assert_eq!(count(&out, "movk x11, #16392, lsl #48"), 0, "the movk is folded away");
    assert_eq!(count(&out, "mov x11, #0"), 0, "the mov#0 is folded away");
}

// TEETH: a top-lane value with frac low bits zero but a NON-imm8 exponent (0xFA0<<48) must NOT
// fold — the exp field fails VFPExpandImm, so no `#imm8` exists; leave the chain untouched.
#[test]
fn fp_imm_declines_non_encodable() {
    let body = "\tmov x11, #0\n\tmovk x11, #4000, lsl #48\n\tfmov d1, x11\n\tmov x11, #5\n";
    let out = fold_fp_imm(body);
    assert_eq!(count(&out, "movk x11, #4000, lsl #48"), 1, "not imm8 ⟹ chain left intact");
}

// TEETH: if xN is READ after the fmov before any redefinition, folding would drop a live GPR
// value — decline. (Here `str x11,[x0]` reads x11 after the bridge.)
#[test]
fn fp_imm_declines_when_base_read_after() {
    let body = "\tmov x11, #0\n\tmovk x11, #16392, lsl #48\n\tfmov d1, x11\n\tstr x11, [x0]\n";
    let out = fold_fp_imm(body);
    assert_eq!(count(&out, "fmov d1, x11"), 1, "x11 live after ⟹ do not fold");
}

// Phase 4.1: xN stays dead THROUGH a writeback epilogue (`ldp …[sp],#16`) that reg_uses flags
// as a boundary but which never names xN — the fold must still fire (the last-constant tail).
#[test]
fn fp_imm_folds_through_writeback_epilogue() {
    let body = "\tmov x11, #0\n\tmovk x11, #16422, lsl #48\n\tfmov d1, x11\n\tfadd d0, d0, d1\n\tldp x29, x30, [sp], #16\n\tret\n";
    let out = fold_fp_imm(body);
    assert_eq!(count(&out, "fmov d1, #11.0"), 1, "11.0 folds; the ldp writeback does not touch x11");
    assert_eq!(count(&out, "movk x11, #16422, lsl #48"), 0);
}

// TEETH: a writeback that DOES name xN (`str w0,[x11],#8`) keeps it live ⟹ decline.
#[test]
fn fp_imm_declines_writeback_on_base() {
    let body = "\tmov x11, #0\n\tmovk x11, #16384, lsl #48\n\tfmov d0, x11\n\tstr w0, [x11], #8\n";
    let out = fold_fp_imm(body);
    assert_eq!(count(&out, "fmov d0, x11"), 1, "x11 is a live writeback base ⟹ do not fold");
}

use super::collapse_fp_bridge;

// Phase 4.3: `fmov x11,d16; fmov d1,x11` (d16→x11→d1) collapses to `fmov d1,d16` reading d16
// directly; the GP hop stays for the dead-def sweep to reap (here it would then be dead).
#[test]
fn fp_bridge_collapses_dxd() {
    let body = "\tfmov x11, d16\n\tfmov d1, x11\n\tfmul d0, d0, d1\n";
    let out = collapse_fp_bridge(body);
    assert_eq!(count(&out, "fmov d1, d16"), 1, "d1 reads d16 directly");
    assert_eq!(count(&out, "fmov d1, x11"), 0, "the GP-sourced copy is rewritten away");
}

// TEETH: the second copy must read the SAME xN the first wrote (xM==xN) — an unrelated
// `fmov x11,d16; fmov d1,x12` is not a bridge and is left untouched.
#[test]
fn fp_bridge_declines_mismatched_reg() {
    let body = "\tfmov x11, d16\n\tfmov d1, x12\n";
    let out = collapse_fp_bridge(body);
    assert_eq!(count(&out, "fmov d1, x12"), 1, "x12 ≠ x11 ⟹ not a bridge ⟹ unchanged");
}

use super::fuse_sxtw_extend;

// Phase 3.1 (sxtw arm): `sxtw xT,wS; add xD,xB,xT,lsl #k` (xD==xT) folds to one extended-add
// `add xD,xB,wS,sxtw #k` — the widen and the shift both vanish.
#[test]
fn sxtw_extend_fuses_scaled_index() {
    let body = "\tsxtw x0, w20\n\tadd x0, x19, x0, lsl #2\n";
    let out = fuse_sxtw_extend(body);
    assert_eq!(count(&out, "add x0, x19, w20, sxtw #2"), 1, "sxtw+shift fold into extend");
    assert_eq!(count(&out, "sxtw x0, w20"), 0);
}

// k=0 (element size 1): a plain `sxtw xT,wS; add xD,xB,xT` folds to `add xD,xB,wS,sxtw`.
#[test]
fn sxtw_extend_k0() {
    let body = "\tsxtw x5, w6\n\tadd x5, x7, x5\n";
    let out = fuse_sxtw_extend(body);
    assert_eq!(count(&out, "add x5, x7, w6, sxtw"), 1, "no shift ⟹ bare sxtw extend");
    assert_eq!(count(&out, "sxtw x5, w6"), 0, "the standalone widen is gone");
}

// TEETH: extend-shift encoding is 0..4 — a larger scale (#5) has no extended-add form, keep.
#[test]
fn sxtw_extend_respects_shift_range() {
    let body = "\tsxtw x0, w1\n\tadd x0, x2, x0, lsl #5\n";
    let out = fuse_sxtw_extend(body);
    assert_eq!(count(&out, "sxtw x0, w1"), 1, "shift 5 > 4 ⟹ no extended-add ⟹ keep");
}

// TEETH: base must differ from the widened index (xB≠xT) or the fold reads the wrong slot.
#[test]
fn sxtw_extend_keeps_when_base_is_index() {
    let body = "\tsxtw x0, w1\n\tadd x0, x0, x0, lsl #2\n";
    let out = fuse_sxtw_extend(body);
    assert_eq!(count(&out, "sxtw x0, w1"), 1, "xB==xT ⟹ no fuse");
}

use super::fuse_shifted_arith;

// Phase 3.1: a scaled index `lsl xT,xM,#s; add xD,xA,xT` with the add overwriting the shift
// dest (xD==xT) fuses to one `add xD,xA,xM,lsl #s`; the lsl is deleted.
#[test]
fn shifted_add_fuses_scaled_index() {
    let body = "\tsxtw x0, w20\n\tlsl x0, x0, #2\n\tadd x0, x19, x0\n";
    let out = fuse_shifted_arith(body);
    assert_eq!(count(&out, "add x0, x19, x0, lsl #2"), 1, "shift folds into the add");
    assert_eq!(count(&out, "lsl x0, x0, #2"), 0, "the lsl is removed");
}

// w-form and sub variants fuse the same way.
#[test]
fn shifted_sub_wform_fuses() {
    let body = "\tlsl w3, w4, #3\n\tsub w3, w5, w3\n";
    let out = fuse_shifted_arith(body);
    assert_eq!(count(&out, "sub w3, w5, w4, lsl #3"), 1);
}

// TEETH: the add must OVERWRITE the shift dest (xD==xT). If xD≠xT the shifted value xT may be
// read later — fusing would drop a live definition. Left untouched.
#[test]
fn shifted_add_keeps_when_dest_differs() {
    let body = "\tlsl x0, x1, #2\n\tadd x2, x19, x0\n";
    let out = fuse_shifted_arith(body);
    assert_eq!(count(&out, "lsl x0, x1, #2"), 1, "xD≠xT ⟹ xT may be live ⟹ no fuse");
    assert_eq!(count(&out, "add x2, x19, x0"), 1);
}

// TEETH: the add's FIRST source must not be the shift dest (xA≠xT) — else the fused add would
// read xT's pre-shift value from the wrong slot.
#[test]
fn shifted_add_keeps_when_first_src_is_shift() {
    let body = "\tlsl x0, x1, #2\n\tadd x0, x0, x0\n";
    let out = fuse_shifted_arith(body);
    assert_eq!(count(&out, "lsl x0, x1, #2"), 1, "xA==xT ⟹ unsafe ⟹ no fuse");
}

use super::fuse_sp_adjust;

// Phase 1.2: two adjacent frame subtractions (prologue fframe + IR temp-spill slab) fuse to
// one — the intermediate sp is never observed, so it is a pure arithmetic identity.
#[test]
fn sp_adjust_fuses_adjacent() {
    let body = "\tmov x29, sp\n\tsub sp, sp, #32\n\tsub sp, sp, #16\n\tmov x19, x0\n";
    let out = fuse_sp_adjust(body);
    assert_eq!(count(&out, "sub sp, sp, #48"), 1, "the two subs fuse to #48");
    assert_eq!(count(&out, "sub sp, sp, #32"), 0);
    assert_eq!(count(&out, "sub sp, sp, #16"), 0);
}

// TEETH: non-adjacent subs (a spill sits between) must NOT fuse — the intermediate sp value
// is live (the spill addresses it), so merging would move the store's target.
#[test]
fn sp_adjust_keeps_nonadjacent() {
    let body = "\tsub sp, sp, #32\n\tstr x19, [sp, #8]\n\tsub sp, sp, #16\n";
    let out = fuse_sp_adjust(body);
    assert_eq!(count(&out, "sub sp, sp, #32"), 1, "non-adjacent ⟹ left as-is");
    assert_eq!(count(&out, "sub sp, sp, #16"), 1);
}

// TEETH: a fused total beyond imm12 (4095) has no single-sub encoding — keep both.
#[test]
fn sp_adjust_respects_imm12() {
    let body = "\tsub sp, sp, #4000\n\tsub sp, sp, #200\n";
    let out = fuse_sp_adjust(body);
    assert_eq!(count(&out, "sub sp, sp, #4000"), 1, "4200 > 4095 ⟹ no fuse");
}

use super::drop_dead_moves;

// DEAD STORE: `mov x24, x0` then x24 is overwritten (`mov x24, x1`) before any read →
// the first store is dead and must be removed; the live second store stays.
#[test]
fn dce_drops_dead_store() {
    let body = "\tmov x24, x0\n\tmov x24, x1\n\tmov x2, x24\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 0, "the overwritten-before-read store is dead");
    assert_eq!(count(&out, "mov x24, x1"), 1, "the store that IS read must stay");
}

// TEETH: a `mov x24, x0` whose value IS read before any overwrite must NOT be dropped —
// deleting it would lose the value. Guards against over-eager DCE (a miscompile).
#[test]
fn dce_keeps_used_store() {
    let body = "\tmov x24, x0\n\tmov x2, x24\n\tmov x24, x1\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 1, "x24 is READ before overwrite ⟹ live, keep");
}

// A read INSIDE a compare/store counts — `str x24,[x1]` reads x24, so the prior store is live.
#[test]
fn dce_counts_reads_in_stores() {
    let body = "\tmov x24, x0\n\tstr x24, [x1]\n\tmov x24, x2\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 1, "str reads x24 ⟹ the store is live");
}

// A region boundary (branch) means all registers are conservatively live-out — a store
// with no in-region overwrite before the branch must be KEPT (it may be read by a successor).
#[test]
fn dce_conservative_across_boundary() {
    let body = "\tmov x24, x0\n\tcbz x1, .Lx\n\tmov x24, x2\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 1, "live-out across a branch ⟹ keep");
}

// Writeback addressing is now MODELLED (pre/post-index base = read+written, Rt = load-write /
// store-read) rather than forcing a boundary. `ldr x2,[x3,#8]!` touches x2/x3, NOT x24, so the
// first `mov x24,x0` is genuinely dead (x24 overwritten by `mov x24,x1` before any read) and
// removable — a sound win the old conservative boundary left on the table.
#[test]
fn dce_models_writeback_base() {
    let body = "\tmov x24, x0\n\tldr x2, [x3, #8]!\n\tmov x24, x1\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 0, "writeback modelled ⟹ x24 dead ⟹ dropped");
    assert_eq!(count(&out, "ldr x2, [x3, #8]!"), 1, "the writeback load itself is preserved");
}

// TEETH for writeback modelling: a `str x24,[x3],#8` READS x24 (post-index store), so the
// prior `mov x24,x0` is LIVE and must be kept — mis-modelling the store's Rt as a write would
// wrongly drop it (a miscompile). The base x3 is read+written; x24 is a pure read.
#[test]
fn dce_writeback_store_reads_rt() {
    let body = "\tmov x24, x0\n\tstr x24, [x3], #8\n\tmov x24, x1\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x24, x0"), 1, "post-index store reads x24 ⟹ keep");
}

// CROSS-BLOCK (Phase 1.4 core): a φ-destruction copy in a loop HEADER whose destination is
// overwritten in the BODY before any read — dead only when liveness crosses the block edge.
// x2 is written by `mov x2,x3` in the header, redefined by `ldrsw x2,[x19]` in the body before
// use, and not a return reg (ret_gp=1 ⟹ only x0 live at exit) ⟹ the header copy is dead.
// The old region-local scan reset to FULL at the label and could never remove it.
#[test]
fn dce_cross_block_dead_phi_copy() {
    let body = "\tmov x0, #0\n.Lh:\n\tmov x2, x3\n\tcmp x19, x20\n\tb.hs .Le\n\
                \tldrsw x2, [x19], #4\n\tadd x0, x0, x2\n\tb .Lh\n.Le:\n\tret\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x2, x3"), 0, "x2 dead across the header→body edge ⟹ dropped");
    assert_eq!(count(&out, "ldrsw x2, [x19], #4"), 1, "the real definition stays");
    assert_eq!(count(&out, "add x0, x0, x2"), 1, "the live accumulation stays");
}

// TEETH for cross-block: the SAME shape but the copy's destination (x0) IS the return value —
// live at `ret` (ret_gp=1) — so it must NOT be dropped even though the body writes x0 too.
#[test]
fn dce_cross_block_keeps_live_out() {
    let body = "\tmov x0, x5\n.Lh:\n\tcmp x19, x20\n\tb.hs .Le\n\
                \tadd x19, x19, #1\n\tb .Lh\n.Le:\n\tret\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x5"), 1, "x0 is returned (live at ret) ⟹ keep");
}

// REGRESSION (the stdarg-1 miscompile): a FLOAT/VECTOR destination whose ADDRESS is a GP
// register — `ldr q0, [x0]` READS x0, it does NOT write it. The `mov x0, xS` feeding the
// address must be KEPT (earlier a positional-parse bug mistook x0 for the destination and
// dropped it, corrupting the load address → SIGABRT).
#[test]
fn dce_keeps_addr_of_float_load() {
    let body = "\tmov x0, x10\n\tldr q0, [x0]\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x10"), 1, "x0 is the load address (read) ⟹ keep");
}

// Same class: `fmov d0, x0` READS x0 (int→float bitcast), does not write it.
#[test]
fn dce_keeps_src_of_fmov_to_float() {
    let body = "\tmov x0, x10\n\tfmov d0, x0\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x10"), 1, "x0 is the fmov source (read) ⟹ keep");
}

// The converse must still work: `fmov x0, d0` WRITES x0 (float→int), so a prior dead store
// to x0 IS dead and removable.
#[test]
fn dce_float_to_gp_writes_dst() {
    let body = "\tmov x0, x10\n\tfmov x0, d0\n\tmov x1, x0\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "mov x0, x10"), 0, "fmov x0,d0 overwrites x0 ⟹ prior store dead");
}

// LEVER 7 (R2): a dead in-place `sxtw x24,w24` (x24 overwritten before any read) is pure
// dead code and removed by the backward-liveness pass.
#[test]
fn dce_drops_dead_inplace_sxtw() {
    let body = "\tsxtw x24, w24\n\tmov x24, x1\n\tmov x2, x24\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "sxtw x24, w24"), 0, "the re-canon whose result is overwritten is dead");
}

// EXHAUSTION: a dead NON-in-place widen `sxtw xD,wS` (D≠S, xD never read — e.g. the memory
// op re-extends wS itself) is equally pure dead code. Global liveness proves xD dead here.
#[test]
fn dce_drops_dead_widen_dest_ne_src() {
    let body = "\tsxtw x20, w1\n\tldrsw x2, [x21, w1, sxtw #2]\n\tadd x0, x0, x2\n\tret\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "sxtw x20, w1"), 0, "x20 never read (op re-extends w1) ⟹ dead");
    assert_eq!(count(&out, "ldrsw x2, [x21, w1, sxtw #2]"), 1, "the real access stays");
}

// TEETH: the widen is KEPT when its dest IS read (here x20 feeds the add) — not dead.
#[test]
fn dce_keeps_live_widen() {
    let body = "\tsxtw x20, w1\n\tadd x0, x0, x20\n\tret\n";
    let out = drop_dead_moves(body, 1);
    assert_eq!(count(&out, "sxtw x20, w1"), 1, "x20 is read by the add ⟹ live ⟹ keep");
}

// Phase 4.2 FP DCE: the poly() write-only-home pattern. `fmov x10,d0; fmov d17,x10` stores the
// op result into an SSA home d17 that is never reloaded (the next op reads d0 directly). Both
// the FP store (d17 dead) and its GP feeder (x10 then dead) are pure dead code — dropped.
#[test]
fn dce_drops_dead_fp_home() {
    let body = "\tfmul d0, d0, d1\n\tfmov x10, d0\n\tfmov d17, x10\n\tfadd d0, d0, d2\n\tret\n";
    let out = drop_dead_moves(body, 0b1);
    assert_eq!(count(&out, "fmov d17, x10"), 0, "d17 never reloaded ⟹ dead FP store");
    assert_eq!(count(&out, "fmov x10, d0"), 0, "x10 dead after its only consumer dropped");
    assert_eq!(count(&out, "fmul d0, d0, d1"), 1, "the real op stays");
    assert_eq!(count(&out, "fadd d0, d0, d2"), 1);
}

// TEETH: the home is KEPT when it IS reloaded (here d17 feeds the fadd) — dropping it would
// lose the value. FP liveness must see the d-register read.
#[test]
fn dce_keeps_live_fp_home() {
    let body = "\tfmov x10, d0\n\tfmov d17, x10\n\tfadd d0, d17, d2\n\tret\n";
    let out = drop_dead_moves(body, 0b1);
    assert_eq!(count(&out, "fmov d17, x10"), 1, "d17 read by the fadd ⟹ live ⟹ keep");
    assert_eq!(count(&out, "fmov x10, d0"), 1, "x10 read by the surviving fmov ⟹ keep");
}

// A float return seeds live-out with d0 (bit 32): the final `fmov d0,d1` producing the result
// must survive. With a GP-only exit mask (no d0) the same copy is provably dead and dropped —
// proving the FP exit-live seed is what keeps it.
#[test]
fn dce_float_return_keeps_d0() {
    let body = "\tfmov d0, d1\n\tret\n";
    assert_eq!(count(&drop_dead_moves(body, 1u64 << 32), "fmov d0, d1"), 1, "d0 is the return ⟹ keep");
    assert_eq!(count(&drop_dead_moves(body, 0b1), "fmov d0, d1"), 0, "d0 not live at exit ⟹ dead");
}

// A dead FP reg-reg copy `fmov d5,d6` (d5 never read) is pure dead code and removed.
#[test]
fn dce_drops_dead_fp_copy() {
    let body = "\tfmov d5, d6\n\tfmov d5, d7\n\tfadd d0, d5, d1\n\tret\n";
    let out = drop_dead_moves(body, 0b1);
    assert_eq!(count(&out, "fmov d5, d6"), 0, "d5 overwritten before read ⟹ dead");
    assert_eq!(count(&out, "fmov d5, d7"), 1, "the copy that IS read stays");
}

use super::drop_wform_sxtw;

// LEVER 7 (R1) CORE: `sxtw x5,w5` followed only by a w-form read then a redefinition — the
// high bits are never observed ⟹ the extension is DEAD and dropped.
#[test]
fn wform_sxtw_dropped_when_only_wform_read() {
    let body = "\tsxtw x5, w5\n\tadd w6, w5, w1\n\tmov w5, w2\n";
    let out = drop_wform_sxtw(body);
    assert_eq!(count(&out, "sxtw x5, w5"), 0, "high bits never read ⟹ sxtw dead");
    assert!(out.contains("add w6, w5, w1"), "the w-form use is untouched");
}

// TEETH: an x-form read of the extended register OBSERVES the high bits ⟹ the sxtw must be
// KEPT (dropping it would corrupt the 64-bit operand — a miscompile).
#[test]
fn wform_sxtw_kept_when_xform_read() {
    let out = drop_wform_sxtw("\tsxtw x5, w5\n\tadd x6, x5, x1\n");
    assert_eq!(count(&out, "sxtw x5, w5"), 1, "x-form read demands the extension ⟹ keep");
}

// An address-index x-form read (`[x0, x5]`) also observes the high bits ⟹ KEEP.
#[test]
fn wform_sxtw_kept_when_used_as_address_index() {
    let out = drop_wform_sxtw("\tsxtw x5, w5\n\tldr x0, [x0, x5]\n");
    assert_eq!(count(&out, "sxtw x5, w5"), 1, "index read is 64-bit ⟹ keep");
}

// A region boundary (branch) before any redefinition leaves the value live-out with unknown
// downstream width ⟹ conservatively KEEP.
#[test]
fn wform_sxtw_kept_across_boundary() {
    let out = drop_wform_sxtw("\tsxtw x5, w5\n\tb .Lx\n");
    assert_eq!(count(&out, "sxtw x5, w5"), 1, "live-out at a boundary ⟹ keep");
}

// The genuine widening `sxtw x5, w2` (D≠S) is an int→long move, never an in-place re-canon —
// the pass must not touch it regardless of how x5 is later used.
#[test]
fn wform_sxtw_ignores_genuine_widening() {
    let out = drop_wform_sxtw("\tsxtw x5, w2\n\tmov w5, w9\n");
    assert_eq!(count(&out, "sxtw x5, w2"), 1, "a widening move is not the in-place form");
}

use super::drop_redundant_uxt;

// LEVER 8 CORE: `ldrb wD` zero-extends bits 8..63, so an in-place `uxtb wD,wD` right after is a
// no-op and is DROPPED. The `uxth` after `ldrb` is likewise redundant (bits ≥16 already zero).
#[test]
fn uxt_dropped_after_byte_load() {
    let out = drop_redundant_uxt("\tldrb w3, [x0]\n\tuxtb w3, w3\n\tadd x2, x2, x3\n");
    assert_eq!(count(&out, "uxtb w3, w3"), 0, "ldrb already zero-extends ⟹ uxtb dead");
    assert!(out.contains("ldrb w3, [x0]"), "the load is untouched");
    let out2 = drop_redundant_uxt("\tldrb w3, [x0]\n\tuxth w3, w3\n");
    assert_eq!(count(&out2, "uxth w3, w3"), 0, "byte-extended ⟹ uxth also a no-op");
}

// TEETH: `uxtb` after `ldrh` is REAL work — the half load leaves bits 8..15, which uxtb clears.
// Must be KEPT (dropping it would change the value).
#[test]
fn uxtb_kept_after_half_load() {
    let out = drop_redundant_uxt("\tldrh w3, [x0]\n\tuxtb w3, w3\n");
    assert_eq!(count(&out, "uxtb w3, w3"), 1, "ldrh leaves bits 8..15 ⟹ uxtb is real");
}

// TEETH: a sign-extending load (`ldrsb`) does NOT zero the high bits, so a following `uxtb`
// is real (it clears the sign fill). Must be KEPT.
#[test]
fn uxt_kept_after_signed_load() {
    let out = drop_redundant_uxt("\tldrsb w3, [x0]\n\tuxtb w3, w3\n");
    assert_eq!(count(&out, "uxtb w3, w3"), 1, "ldrsb sign-extends ⟹ uxtb is not a no-op");
}

// TEETH: an intervening write of the register between the load and the uxt clears the known-zero
// floor ⟹ the uxt is no longer provably redundant. Must be KEPT.
#[test]
fn uxt_kept_when_reg_rewritten_between() {
    let out = drop_redundant_uxt("\tldrb w3, [x0]\n\tadd w3, w4, w5\n\tuxtb w3, w3\n");
    assert_eq!(count(&out, "uxtb w3, w3"), 1, "reg redefined ⟹ floor lost ⟹ keep");
}

use super::drop_redundant_sxtw;

// LEVER 2 exhaustion (#19 residency): `and Rd,Rn,#imm` with imm ≤ 0x7fffffff produces a
// non-negative int32 (bits 31..63 all 0) ⟹ it is already sign-canonical ⟹ an in-place `sxtw`
// right after is a proven no-op and is DROPPED. This is the histogram/popcount hot-loop case.
#[test]
fn sxtw_dropped_after_and_small_mask() {
    let out = drop_redundant_sxtw("\tand x1, x1, #255\n\tsxtw x1, w1\n\tlsl x2, x1, #2\n");
    assert_eq!(count(&out, "sxtw x1, w1"), 0, "and #255 ⟹ x1 ∈ [0,255] ⟹ sxtw is a no-op");
    assert!(out.contains("and x1, x1, #255") && out.contains("lsl x2, x1, #2"), "rest untouched");
    // w-form and, same slot: dst write zero-extends AND bit 31 = 0 ⟹ canonical.
    let out2 = drop_redundant_sxtw("\tand w0, w0, #8191\n\tsxtw x0, w0\n");
    assert_eq!(count(&out2, "sxtw x0, w0"), 0, "and #8191 (13-bit mask) ⟹ canonical");
}

// TEETH: a mask with bit 31 set (≥ 0x80000000) can leave a negative int32 ⟹ NOT sign-canonical
// (bits 32..63 are 0 but bit 31 may be 1, so sxtw would sign-fill differently). Must be KEPT.
#[test]
fn sxtw_kept_after_and_high_bit_mask() {
    let out = drop_redundant_sxtw("\tand x1, x1, #0xffffffff\n\tsxtw x1, w1\n");
    assert_eq!(count(&out, "sxtw x1, w1"), 1, "0xffffffff mask ⟹ bit 31 may be set ⟹ keep");
    // register-form `and` (no immediate) has an unknown result ⟹ not canonical.
    let out2 = drop_redundant_sxtw("\tand x1, x1, x9\n\tsxtw x1, w1\n");
    assert_eq!(count(&out2, "sxtw x1, w1"), 1, "and Rd,Rn,Rm has unknown high bits ⟹ keep");
}

// TEETH: `uxtb`/`uxth` and `ubfx …,#width≤31` are non-negative-int32 producers too.
#[test]
fn sxtw_dropped_after_uxt_and_ubfx() {
    assert_eq!(count(&drop_redundant_sxtw("\tuxtb w3, w3\n\tsxtw x3, w3\n"), "sxtw x3, w3"), 0);
    assert_eq!(count(&drop_redundant_sxtw("\tubfx x4, x5, #3, #10\n\tsxtw x4, w4\n"), "sxtw x4, w4"), 0);
    // width 32 ⟹ result may have bit 31 set ⟹ keep.
    assert_eq!(count(&drop_redundant_sxtw("\tubfx x4, x5, #0, #32\n\tsxtw x4, w4\n"), "sxtw x4, w4"), 1);
}

// TEETH: an intervening redefinition of the register clears its canonical status ⟹ sxtw KEPT.
#[test]
fn sxtw_kept_when_reg_rewritten_after_and() {
    let out = drop_redundant_sxtw("\tand x1, x1, #255\n\tadd x1, x4, x5\n\tsxtw x1, w1\n");
    assert_eq!(count(&out, "sxtw x1, w1"), 1, "reg redefined by add ⟹ canonical lost ⟹ keep");
}

use super::{cbz_fuse, post_index};

// LEVER 5 CORE: `ldr x5,[x6]` + `add x6,x6,#8` (base incremented, no intervening use) folds
// into a post-index `ldr x5,[x6],#8`, the add deleted.
#[test]
fn post_index_folds_increment() {
    let out = post_index("\tldr x5, [x6]\n\tadd x6, x6, #8\n");
    assert!(out.contains("ldr x5, [x6], #8"), "the increment folds into the access");
    assert_eq!(count(&out, "add x6, x6, #8"), 0, "the separate add is deleted");
}

// TEETH: `ldr x6,[x6]` (loaded value overwrites the base) is UNPREDICTABLE as a post-index —
// must NOT fold; the add stays.
#[test]
fn post_index_declines_load_into_base() {
    let out = post_index("\tldr x6, [x6]\n\tadd x6, x6, #8\n");
    assert_eq!(count(&out, "add x6, x6, #8"), 1, "load-into-base cannot post-index");
}

// TEETH: an intervening READ of the base before the increment means the un-incremented base is
// observed ⟹ must NOT fold.
#[test]
fn post_index_declines_when_base_used_between() {
    let out = post_index("\tldr x5, [x6]\n\tadd x9, x6, x2\n\tadd x6, x6, #8\n");
    assert!(out.contains("ldr x5, [x6]\n"), "the access is not post-indexed");
    assert_eq!(count(&out, "add x6, x6, #8"), 1, "base read before increment ⟹ no fold");
}

// TEETH: a label between access and increment is a merge point — the increment may be shared;
// folding it would lose a predecessor's advance (the ssad-run bug). Must NOT fold.
#[test]
fn post_index_declines_across_label() {
    let out = post_index("\tldr x5, [x6]\n.Lx:\n\tadd x6, x6, #8\n");
    assert_eq!(count(&out, "add x6, x6, #8"), 1, "increment past a label is a boundary ⟹ keep");
}

// LEVER 6 CORE: `cmp x5,#0` + `b.eq L` collapses to `cbz x5, L`, deleting the cmp; `b.ne`→cbnz.
#[test]
fn cbz_fuse_collapses_eq_and_ne() {
    let eq = cbz_fuse("\tcmp x5, #0\n\tb.eq .Lx\n\tmov x0, x1\n");
    assert!(eq.contains("cbz x5, .Lx"), "cmp #0 + b.eq ⟹ cbz");
    assert_eq!(count(&eq, "cmp x5, #0"), 0, "the cmp is deleted");
    let ne = cbz_fuse("\tcmp w3, #0\n\tb.ne .Ly\n\tret\n");
    assert!(ne.contains("cbnz w3, .Ly"), "cmp #0 + b.ne ⟹ cbnz");
}

// TEETH: a later flag-reader on the fall-through (`cset` reads NZCV) means the cmp's flags are
// still LIVE ⟹ the cmp must NOT be deleted.
#[test]
fn cbz_fuse_declines_when_flags_live() {
    let out = cbz_fuse("\tcmp x5, #0\n\tb.eq .Lx\n\tcset w0, gt\n");
    assert_eq!(count(&out, "cmp x5, #0"), 1, "flags read after ⟹ cmp still needed ⟹ keep");
}
