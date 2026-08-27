// The §4 pass batteries.
//
// Two obligations per pass, and the second is the one that is usually skipped:
//
//   (1) SOUNDNESS — ⟦f⟧ = ⟦P f⟧. The whole `hir::tests` corpus already runs both
//       sides of this square on every program in it (see `hir/tests.rs::check`),
//       so what lives here is the targeted half: programs written specifically
//       to make one pass fire on one shape.
//
//   (2) EFFECT — the pass actually fires. A commuting square holds VACUOUSLY for
//       a pass that does nothing, so each battery below also asserts the
//       STRUCTURAL consequence (an instruction count that drops, a block that
//       disappears, a branch that becomes a jump). Law 4: a green test that
//       cannot distinguish "correct" from "absent" is not evidence.
use super::super::interp::{Trap, new_machine};
use super::super::{BinOp, BlockId, Func, Inst, Module, Operand, Term, build, verify};
use crate::testutil::frontend;

fn module(src: &str, opt: bool) -> Module {
    let ast = frontend(src);
    let mut m = build::build(&ast);
    if opt {
        super::run_module_with(&mut m, &crate::compile::pinned_symbols(&ast));
    }
    for f in &m.funcs {
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    m
}

fn run(m: &Module, ast: &crate::ast::Ast) -> Result<i64, Trap> {
    let mut mach = new_machine(m, ast);
    mach.call("main", &[]).map(|r| r.unwrap_or(0) as i32 as i64)
}

/// ⟦f⟧ = ⟦P f⟧ on this program, and both sides equal `want`.
fn square(src: &str, want: i64) {
    let ast = frontend(src);
    let mut a = build::build(&ast);
    let mut b = a.clone();
    super::run_module_with(&mut b, &crate::compile::pinned_symbols(&ast));
    for f in &b.funcs {
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let ra = run(&mut a, &ast);
    let rb = run(&b, &ast);
    match (ra, rb) {
        (Ok(x), Ok(y)) if x == y && x == want => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦P f⟧={:?} want {}\n{}", x, y, want, src),
    }
}

fn func<'a>(m: &'a Module, name: &str) -> &'a Func {
    m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no {}", name))
}

fn ninsts(f: &Func) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}

fn nlive_blocks(f: &Func) -> usize {
    let c = super::dom::cfg(f);
    (0..f.blocks.len()).filter(|b| c.reachable(*b as u32)).count()
}

fn count(f: &Func, p: impl Fn(&Inst) -> bool) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| p(i)).count()
}

// ── fold: the rewrite table, exhaustively over its own rows ────────────────

#[test]
fn fold_table_is_an_identity_on_the_full_type() {
    use super::super::{BinOp::*, Operand::*, Ty};
    #[allow(unused_imports)]
    use super::super::BinOp as _B;
    use super::super::interp::eval_bin;
    // Every algebraic row of `fold.rs`, checked against ⟦·⟧ at a boundary sample
    // of the FULL type rather than at a convenient midpoint (Article E).
    let samples: [i64; 9] = [0, 1, -1, 2, -2, 127, -128, i32::MAX as i64, i32::MIN as i64];
    let rows: [(BinOp, i64, i64); 12] = [
        (Add, 0, -1),
        (Sub, 0, -1),
        (Or, 0, -1),
        (Xor, 0, -1),
        (Shl, 0, -1),
        (LShr, 0, -1),
        (AShr, 0, -1),
        (Mul, 0, 1),
        (And, 0, -1),
        (SDiv, 1, -1),
        (UDiv, 1, -1),
        (SRem, 1, -1),
    ];
    for ty in [Ty::I32, Ty::I64] {
        for (op, k, _) in rows {
            for x in samples {
                let a = Val(0);
                let folded = super::fold::fold_inst(&Inst::Bin {
                    dst: 1,
                    op,
                    ty,
                    a,
                    b: Imm(k),
                });
                let direct = eval_bin(op, ty, x as u64, k as u64).unwrap();
                let via = match folded {
                    Some(Val(_)) => x as u64,
                    Some(Imm(c)) => c as u64,
                    _ => continue,
                };
                assert_eq!(
                    super::super::interp::mask(via, ty),
                    super::super::interp::mask(direct, ty),
                    "{:?} {:?} x={} k={}",
                    op,
                    ty,
                    x,
                    k
                );
            }
        }
    }
}

// ── cfg_simplify ───────────────────────────────────────────────────────────

#[test]
fn cfg_simplify_merges_and_threads() {
    let src = "int main(void){int a=1;int b=2;int c=3;return a+b+c;}";
    let before = module(src, false);
    let after = module(src, true);
    // A straight-line function is built as several blocks (each statement can
    // seal one); after simplification it is a single block.
    assert!(nlive_blocks(func(&before, "main")) >= 1);
    assert_eq!(nlive_blocks(func(&after, "main")), 1, "straight line must collapse to one block");
    square(src, 6);
}

#[test]
fn cfg_simplify_deletes_the_untaken_arm() {
    let src = "int main(void){if(0){return 1;}return 2;}";
    let after = module(src, true);
    let f = func(&after, "main");
    assert_eq!(nlive_blocks(f), 1, "a constant `if` must leave one block");
    for b in &f.blocks {
        assert!(!matches!(b.term, Term::Br(..)), "the branch must be gone");
    }
    square(src, 2);
}

#[test]
fn cfg_simplify_keeps_a_labelled_block_addressable() {
    // EXT(gcc) `&&label` + `goto *`: the block's IDENTITY is observable, so no
    // merge or thread may absorb it. The test is that the program still runs —
    // a lost label symbol is a link error, and a merged one is a wrong jump.
    let src = "int main(void){void*p=&&L;int x=0;goto *p;L:x=7;return x;}";
    square(src, 7);
}

// ── sccp ───────────────────────────────────────────────────────────────────

#[test]
fn sccp_meets_a_join_parameter_both_edges_agree_on() {
    // Both arms of the conditional yield 7, so the join PARAMETER is 7 — a fact
    // no expression-level pass can see (the parameter is not an expression) and
    // no control-flow pass can see (the two blocks are genuinely distinct). It
    // is exactly the lattice meet, which is why this program isolates sccp.
    let src = "int f(int n){if((n?7:3+4)==7)return 42;return 0;}int main(void){return f(1);}";
    let after = module(src, true);
    let f = func(&after, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Cmp { .. })), 0, "t==7 must fold");
    square(src, 42);
    // and the same shape when the disagreement is real: nothing may be folded
    let live = "int f(int n){if((n?7:8)==7)return 42;return 9;}int main(void){return f(0);}";
    assert!(count(func(&module(live, true), "f"), |i| matches!(i, Inst::Cmp { .. })) >= 1);
    square(live, 9);
}

#[test]
fn sccp_kills_the_arm_a_constant_makes_unreachable() {
    let src = "int f(int n){if((n?2:1+1)>3){return n*0;}return n+1;}int main(void){return f(41);}";
    let after = module(src, true);
    let f = func(&after, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Cmp { .. })), 0, "the compare must be folded away");
    assert_eq!(
        count(f, |i| matches!(i, Inst::Bin { op: BinOp::Mul, .. })),
        0,
        "the unreachable arm and its multiply must be gone"
    );
    square(src, 42);
}

#[test]
fn sccp_does_not_fold_a_trapping_division() {
    // C99 6.5.5p5 makes `1/0` undefined; the folder declines it, so the fault
    // stays exactly where C put it instead of becoming a silent constant.
    let src = "int main(void){int z=0;if(z)return 1/z;return 42;}";
    square(src, 42);
    let m = module("int f(int q){int z=0;return q/z;}int main(void){return 42;}", true);
    let f = func(&m, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. })), 1);
}

// ── gvn ────────────────────────────────────────────────────────────────────

#[test]
fn gvn_numbers_a_repeated_expression_once() {
    // Four references to `g` build four `SymAddr` values and two duplicated
    // index computations; all of them are pure, so one of each survives. On
    // AArch64 each removed `SymAddr` is an `adrp`/`add` pair, which is why this
    // is worth a pass rather than a peephole.
    let src = "int g[4];int main(void){g[1]=1;g[2]=2;return g[1]+g[2]+39;}";
    let before = module(src, false);
    let after = module(src, true);
    let sym = |f: &Func| count(f, |i| matches!(i, Inst::SymAddr { .. }));
    let nb = sym(func(&before, "main"));
    let na = sym(func(&after, "main"));
    assert!(nb >= 4, "the source really does name `g` four times: {}", nb);
    assert_eq!(na, 1, "gvn must leave exactly one address of `g` (was {})", nb);
    square(src, 42);
    // The memory-carried form of this test — two spellings of `a*b` on locals —
    // needs the locals promoted out of the frame first; it is R2.2's battery.
}

#[test]
fn gvn_dedups_the_address_of_a_local() {
    // The R1 ground metric's headline: every local access starts with a
    // `SlotAddr`. Numbering them is the first bite out of the 28.2% `add` share.
    let src = "int main(void){int a[4];a[0]=1;a[1]=2;a[2]=3;a[3]=4;return a[0]+a[1]+a[2]+a[3];}";
    let before = module(src, false);
    let after = module(src, true);
    let nb = count(func(&before, "main"), |i| matches!(i, Inst::SlotAddr { .. }));
    let na = count(func(&after, "main"), |i| matches!(i, Inst::SlotAddr { .. }));
    assert!(na < nb, "slot addresses must collapse: {} -> {}", nb, na);
    square(src, 10);
}

#[test]
fn gvn_respects_dominance() {
    // The expression in the `then` arm does NOT dominate the one after the `if`,
    // so numbering must not reuse it — the whole point of the dominator scope.
    let src = "int f(int c,int a,int b){int t=0;if(c){t=a/b;}return t+a/b;}int main(void){return f(0,84,2);}";
    square(src, 42);
}

// ── sroa + mem2reg ─────────────────────────────────────────────────────────

#[test]
fn mem2reg_promotes_a_scalar_local() {
    let src = "int main(void){int i,s=0;for(i=0;i<10;i++)s+=i;return s;}";
    let before = module(src, false);
    let after = module(src, true);
    let mem = |f: &Func| {
        count(f, |i| matches!(i, Inst::Load { .. } | Inst::Store { .. } | Inst::SlotAddr { .. }))
    };
    assert!(mem(func(&before, "main")) > 0, "R1 keeps every local in the frame");
    assert_eq!(
        mem(func(&after, "main")),
        0,
        "a local whose address is never taken must leave memory entirely"
    );
    square(src, 45);
}

#[test]
fn sroa_splits_a_struct_field_by_field() {
    // The address of the struct is never taken, so each FIELD is its own
    // promotable piece — that is the whole of SROA on this architecture: the
    // unit of promotion is an (offset, type) piece, not an object.
    let src = "struct P{int x;int y;};int main(void){struct P p;p.x=17;p.y=25;return p.x+p.y;}";
    let after = module(src, true);
    let f = func(&after, "main");
    assert_eq!(
        count(f, |i| matches!(i, Inst::Load { .. } | Inst::Store { .. })),
        0,
        "both fields must be promoted"
    );
    square(src, 42);
}

#[test]
fn an_escaped_local_bounds_the_escape_to_its_own_object() {
    // `&a` escapes, so `a` stays in memory — but `b` is a different object and
    // C99 6.5.6p8 says no arithmetic on `&a` can reach it, so `b` promotes. The
    // whole point of exporting the parser's object table: without extents, one
    // escaped local would pin the entire frame.
    // `&a` is stored into a global, so nothing can bound the escape but the
    // object table.
    let src = "int *gp;int main(void){int a=1;int b=2;gp=&a;*gp=5;return a*8+b;}";
    let after = module(src, true);
    let f = func(&after, "main");
    // Exactly ONE stack object is still addressed — `a`, whose address escaped.
    // `b` never touches memory, which is the whole point: without the parser's
    // extents the escape would have had to pin the entire frame.
    assert_eq!(
        count(f, |i| matches!(i, Inst::SlotAddr { .. })),
        1,
        "only the escaped object keeps a stack address"
    );
    square(src, 42);
}

#[test]
fn a_type_punned_slot_is_not_promoted() {
    // Two different widths at one offset are not a variable; promoting either
    // would lose the other's bytes. The union below is the C spelling of it.
    let src = "union U{int i;char c[4];};int main(void){union U u;u.i=0x2a;return u.c[0];}";
    square(src, 42);
}

#[test]
fn a_variably_indexed_array_stays_in_memory() {
    // `a[i]` computes an address from the slot address, which is an escape of
    // that object — and the object is the whole array, so none of it promotes.
    let src = "int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i*i;return a[0]+a[1]+a[2]+a[3]+28;}";
    square(src, 42);
}

#[test]
fn a_pointer_local_dead_across_a_loop_is_promoted_by_pruned_ssa() {
    // Regression (csmith c2331 miscompile / c6148 SIGSEGV). A pointer-valued local
    // `p` is defined from a global chain, dereferenced to read and write, and dead
    // — all within one loop-body block. Its store is a pointer written and read
    // back inside that block; nothing reads `p` on any later block. MINIMAL SSA
    // (parameters at the whole iterated dominance frontier of the stores) threaded
    // its value through the loop header as a block parameter no one reads; that
    // dead loop-carried parameter is a no-op in HIR but the backend miscompiled it.
    // PRUNED SSA gates placement on liveness, so a piece live in a single block
    // gets no parameter. The commuting square (opt == -O0 == the reference exit)
    // is the proof; g = 3 + (0+1+..+6) = 24.
    let src = "int main(void){int g=3;int*q=&g;int i;\
               for(i=0;i<7;i++){int*p=q;int v=*p;*p=v+i;}return g&0x7f;}";
    square(src, 24);
}

#[test]
fn gvn_sees_through_a_promoted_local() {
    // The R2.1 form of this test could only dedup a global's address, because
    // every local was a memory cell and two reads of it were two loads. After
    // promotion the two spellings of `a*b` are the same expression.
    let src = "int f(int a,int b){int x=a*b+7;int y=a*b+7;return x+y;}int main(void){return f(3,5);}";
    let before = module(src, false);
    let after = module(src, true);
    let muls = |f: &Func| count(f, |i| matches!(i, Inst::Bin { op: BinOp::Mul, .. }));
    assert_eq!(muls(func(&before, "f")), 2);
    assert_eq!(muls(func(&after, "f")), 1, "gvn must leave exactly one product");
    square(src, 44);
}

#[test]
fn sccp_finds_a_constant_around_a_loop() {
    // `k` is 1 on entry and 1 on the back edge, so the meet at the loop header
    // is 1 — a fact no non-conditional folder can see, because the back edge is
    // only known executable while the analysis is still running. It needs
    // mem2reg first: while `k` is a memory cell there is no loop-carried VALUE
    // to run a lattice over.
    let src = "int main(void){int k=1;int i;for(i=0;i<10;i++){k=k*1;}return k+41;}";
    let after = module(src, true);
    let f = func(&after, "main");
    assert_eq!(
        count(f, |i| matches!(i, Inst::Bin { op: BinOp::Mul, .. })),
        0,
        "the loop-carried multiply must be gone"
    );
    assert!(
        f.blocks.iter().any(|b| matches!(&b.term, Term::Ret(Some(Operand::Imm(42))))),
        "and the result must be the constant 42, not a loop-carried value"
    );
    // The loop ITSELF survives: deleting a counted loop with no effect needs a
    // final-value/loop-DCE theorem, which is R2.4's row, not sccp's.
    square(src, 42);
}

// ── load_elim / dse ────────────────────────────────────────────────────────

#[test]
fn a_stored_value_is_forwarded_to_the_load() {
    let src = "int f(int*p){*p=42;return *p;}int main(void){int x=0;return f(&x);}";
    let after = module(src, true);
    let f = func(&after, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Load { .. })), 0, "the reload of *p is redundant");
    assert_eq!(count(f, |i| matches!(i, Inst::Store { .. })), 1, "but the store is observable");
    square(src, 42);
}

#[test]
fn a_second_read_of_the_same_place_is_the_first() {
    let src = "int f(int*p){return *p + *p;}int main(void){int x=21;return f(&x);}";
    let after = module(src, true);
    assert_eq!(count(func(&after, "f"), |i| matches!(i, Inst::Load { .. })), 1);
    square(src, 42);
}

#[test]
fn an_overwritten_store_is_dead() {
    let src = "int f(int*p){*p=1;*p=42;return 0;}int main(void){int x=0;f(&x);return x;}";
    let after = module(src, true);
    assert_eq!(
        count(func(&after, "f"), |i| matches!(i, Inst::Store { .. })),
        1,
        "only the last store to a location is observable"
    );
    square(src, 42);
}

#[test]
fn a_call_between_the_store_and_the_load_blocks_forwarding() {
    // The callee may hold a pointer to the same object, so the reload has to
    // happen. This is the case a "forward anything" rule miscompiles.
    let src = "int*g;void set(void){*g=42;}               int f(int*p){*p=1;set();return *p;}               int main(void){int x=0;g=&x;return f(&x);}";
    let after = module(src, true);
    assert!(
        count(func(&after, "f"), |i| matches!(i, Inst::Load { .. })) >= 1,
        "an opaque call must invalidate the table"
    );
    square(src, 42);
}

#[test]
fn a_volatile_access_is_never_forwarded_or_removed() {
    let src = "int f(volatile int*p){*p=42;return *p;}int main(void){int x=0;return f(&x);}";
    let after = module(src, true);
    let f = func(&after, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Load { vol: true, .. })), 1);
    assert_eq!(count(f, |i| matches!(i, Inst::Store { vol: true, .. })), 1);
    square(src, 42);
}

#[test]
fn disjoint_objects_do_not_kill_each_other() {
    // Writing through one pointer must not invalidate what is known about a
    // different object — that is the whole value of the oracle.
    let src = "int g1;int g2;               int f(void){g1=42;g2=7;return g1;}               int main(void){return f();}";
    let after = module(src, true);
    assert_eq!(
        count(func(&after, "f"), |i| matches!(i, Inst::Load { .. })),
        0,
        "g2's store cannot touch g1"
    );
    square(src, 42);
}

// ── dce ────────────────────────────────────────────────────────────────────

#[test]
fn dce_removes_an_unused_computation_but_not_a_call() {
    // `g` is recursive, so the inliner never substitutes it and the call is
    // still a call when dce runs.
    let src = "int g(int n){if(n>0)return g(n-1);return 1;}\
               int main(void){int dead=7*13;int keep=g(3);return keep+41;}";
    let after = module(src, true);
    let f = func(&after, "main");
    assert_eq!(count(f, |i| matches!(i, Inst::Call { .. })), 1, "a call is never dead");
    assert_eq!(count(f, |i| matches!(i, Inst::Bin { op: BinOp::Mul, .. })), 0);
    square(src, 42);
}

#[test]
fn dce_keeps_a_volatile_access() {
    let src = "int main(void){volatile int v=5;int x=v;(void)x;return 42;}";
    let after = module(src, true);
    let f = func(&after, "main");
    assert!(
        count(f, |i| matches!(i, Inst::Load { vol: true, .. })) >= 1,
        "C99 6.7.3: a volatile read may not be deleted"
    );
    square(src, 42);
}

#[test]
fn dce_drops_a_dead_block_parameter() {
    let src = "int main(void){int i;int s=0;int unused=0;for(i=0;i<5;i++){s+=i;unused+=i*i;}return s+32;}";
    square(src, 42);
}

// ── licm ───────────────────────────────────────────────────────────────────

#[test]
fn licm_hoists_an_invariant_expression_out_of_the_loop() {
    let src = "int f(int a,int b,int n){int i,s=0;for(i=0;i<n;i++)s+=a*b;return s;}               int main(void){return f(3,2,7);}";
    let after = module(src, true);
    let f = func(&after, "f");
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    assert_eq!(lf.loops.len(), 1, "the loop must still be a loop");
    let body = &lf.loops[0].body;
    let in_loop = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(b, _)| body.contains(&(*b as u32)))
        .flat_map(|(_, blk)| blk.insts.iter())
        .filter(|i| matches!(i, Inst::Bin { op: BinOp::Mul, .. }))
        .count();
    assert_eq!(in_loop, 0, "a*b is invariant and must leave the body");
    square(src, 42);
}

#[test]
fn licm_refuses_a_division_that_could_trap() {
    // C99 6.5.5p5: division by zero is undefined. Hoisting `a/b` out of a loop
    // that never runs would move the fault onto a path the program never took —
    // so it moves only when the divisor is a non-zero literal.
    let src = "int f(int a,int b,int n){int i,s=0;for(i=0;i<n;i++)s+=a/b;return s;}               int main(void){return f(6,3,0)+42;}";
    let after = module(src, true);
    let f = func(&after, "f");
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    let body = &lf.loops[0].body;
    let in_loop = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(b, _)| body.contains(&(*b as u32)))
        .flat_map(|(_, blk)| blk.insts.iter())
        .filter(|i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. }))
        .count();
    assert_eq!(in_loop, 1, "a/b may fault and must stay inside");
    square(src, 42);
    // a literal divisor cannot fault, so that one does move
    let ok = "int f(int a,int n){int i,s=0;for(i=0;i<n;i++)s+=a/3;return s;}              int main(void){return f(6,7);}";
    square(ok, 14);
}

#[test]
fn licm_leaves_a_variant_expression_alone() {
    let src = "int f(int a,int n){int i,s=0;for(i=0;i<n;i++)s+=a*i;return s;}               int main(void){return f(2,7);}";
    square(src, 42);
}

// ── the ladder as a whole ──────────────────────────────────────────────────

#[test]
fn ladder_is_idempotent_at_the_fixpoint() {
    // Running the ladder twice must produce the same instruction count as once:
    // if a second run still finds work, the ROUNDS bound is truncating real
    // optimization rather than confirming a fixpoint (Law 4, convenience vs
    // fundamental).
    for src in [
        "int main(void){int i,s=0;for(i=0;i<10;i++)s+=i*2+1;return s;}",
        "int f(int*p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}int main(void){int a[3];a[0]=1;a[1]=2;a[2]=3;return f(a,3);}",
        "int main(void){int a=1,b=2,c=3;while(a<100){a=a+b+c;b=a-b;}return a>b;}",
    ] {
        let ast = frontend(src);
        let mut m = build::build(&ast);
        // ⟦f⟧ = ⟦ladder f⟧ FIRST. Idempotence alone is a statement about the
        // ROUNDS bound, not about meaning: a ladder that miscompiled
        // identically on both runs would satisfy it. `provenance.sh` caught
        // that this square had the effect half and not the equivalence half.
        let before = run(&m, &ast);
        super::run_module(&mut m);
        let once: Vec<usize> = m.funcs.iter().map(ninsts).collect();
        let after = run(&m, &ast);
        assert_eq!(before, after, "⟦f⟧ ≠ ⟦ladder f⟧\n{}", src);
        super::run_module(&mut m);
        let twice: Vec<usize> = m.funcs.iter().map(ninsts).collect();
        assert_eq!(once, twice, "the ladder had not reached its fixpoint\n{}", src);
        assert_eq!(after, run(&m, &ast), "⟦ladder f⟧ ≠ ⟦ladder² f⟧\n{}", src);
    }
}

// ── inline ─────────────────────────────────────────────────────────────────

#[test]
fn inline_substitutes_a_static_callee_called_once() {
    let src = "static int helper(int a,int b){int t=a*b;return t+t;}\
               int main(void){return helper(3,7);}";
    let after = module(src, true);
    assert_eq!(count(func(&after, "main"), |i| matches!(i, Inst::Call { .. })), 0);
    assert!(
        after.funcs.iter().all(|f| f.name != "helper"),
        "a static callee with no remaining call site is dead and must not be emitted"
    );
    square(src, 42);
}

#[test]
fn inline_refuses_a_recursive_callee() {
    // β-reduction on a recursive term does not terminate; the pass must see the
    // cycle in the call graph and decline.
    let src = "int fact(int n){if(n<2)return 1;return n*fact(n-1);}\
               int main(void){return fact(5)-78;}";
    let after = module(src, true);
    assert!(
        count(func(&after, "fact"), |i| matches!(i, Inst::Call { .. })) >= 1,
        "the recursive call must survive"
    );
    square(src, 42);
}

#[test]
fn inline_refuses_a_variadic_callee() {
    let src = "static int sum(int n,...){__builtin_va_list a;int i,s=0;\
               __builtin_va_start(a,n);for(i=0;i<n;i++)s+=__builtin_va_arg(a,int);\
               __builtin_va_end(a);return s;}\
               int main(void){return sum(3,20,20,2);}";
    let after = module(src, true);
    assert!(after.funcs.iter().any(|f| f.name == "sum"), "a variadic callee stays a call");
    square(src, 42);
}

#[test]
fn inline_keeps_a_function_a_static_initializer_names() {
    // The reference lives in the data segment, where HIR cannot see it; deleting
    // the body would be a link error, not a size win.
    let src = "static int one(void){return 1;}\
               static int (*tab[1])(void) = { one };\
               int main(void){return tab[0]()+41;}";
    let after = module(src, true);
    assert!(after.funcs.iter().any(|f| f.name == "one"));
    // Not run under ⟦hir⟧: the interpreter does not materialize a FUNCTION
    // address inside static data, so the table would read as a null pointer.
    // The obligation here is structural — the body must still exist — and the
    // end-to-end confirmation is `tests/cases`, which links.
}

#[test]
fn inline_preserves_the_value_through_a_branchy_callee() {
    let src = "static int pick(int c,int a,int b){if(c)return a;return b;}\
               int main(void){int s=0;s+=pick(1,40,0);s+=pick(0,0,2);return s;}";
    square(src, 42);
}

#[test]
fn an_aliased_global_is_one_object_under_two_names() {
    // EXT(gcc) `__attribute__((alias))` declares another NAME, not another
    // object. The oracle's rule "different symbols are different objects" is
    // true of C but not of the linker, so the alias is resolved to its target
    // when the address is built — otherwise a store through one name is
    // invisible to a load through the other (torture `alias-3`).
    let src = "static int a=0;extern int b __attribute__((alias(\"a\")));\
               int main(void){a=0;b++;return a==1?42:0;}";
    let after = module(src, true);
    let f = func(&after, "main");
    let stores = count(f, |i| matches!(i, Inst::Store { .. }));
    assert!(stores >= 1, "the increment through the alias is observable");
    square(src, 42);
}

// ── if_convert ─────────────────────────────────────────────────────────────

#[test]
fn ifconv_turns_a_diamond_into_a_select() {
    let src = "int f(int a,int b){int m;if(a<b)m=a;else m=b;return m;}\
               int main(void){return f(42,50);}";
    let after = module(src, true);
    let f = func(&after, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Select { .. })), 1, "the diamond must become a select");
    assert_eq!(nlive_blocks(f), 1, "and the branch must be gone");
    square(src, 42);
}

#[test]
fn ifconv_refuses_a_side_effecting_arm() {
    // A store on one side is not speculatable: executing it unconditionally
    // would write on a path that never wrote.
    let src = "int g;int f(int c){if(c)g=1;return g;}int main(void){g=42;return f(0);}";
    let after = module(src, true);
    assert!(
        count(func(&after, "f"), |i| matches!(i, Inst::Store { .. })) >= 1,
        "the store must stay, and stay conditional"
    );
    square(src, 42);
}

#[test]
fn ifconv_refuses_a_faulting_arm() {
    // C99 6.5.5p5: `a/b` with b == 0 is undefined, so it may not be speculated
    // onto the path that avoided it.
    let src = "int f(int a,int b){int r;if(b)r=a/b;else r=42;return r;}\
               int main(void){return f(1,0);}";
    let after = module(src, true);
    assert!(
        count(func(&after, "f"), |i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. })) >= 1
            && nlive_blocks(func(&after, "f")) > 1,
        "the division must stay behind its branch"
    );
    square(src, 42);
}

#[test]
fn ifconv_leaves_a_float_diamond_to_the_branch() {
    // `fcsel` is a different instruction on a different register file, and MIR
    // has no form for it — recorded as a residual rather than mis-selected.
    let src = "double f(int c,double a,double b){double r;if(c)r=a;else r=b;return r;}\
               int main(void){return (int)f(1,42.0,0.0);}";
    square(src, 42);
}

#[test]
fn a_select_of_one_and_zero_is_not_its_condition() {
    // `select c, 1, 0` tests `c ≠ 0` exactly as a branch does, so it equals `c`
    // only when `c` is ALREADY 0 or 1. `x && y` supplies a whole value, and
    // rewriting it away returned 3 where C99 6.5.13 says 1 (torture pr10352-1).
    // (The torture case itself uses a static initializer holding an ADDRESS,
    // which ⟦hir⟧ does not materialize — the end-to-end confirmation for that
    // shape is the suite. What is checkable here is the rewrite itself.)
    let s2 = "int f(int x){int t = x?1:0;return t;}int main(void){return f(3)==1?42:0;}";
    square(s2, 42);
    let s3 = "int f(int x,int y){return (x && y) == 1;}int main(void){return f(3,5)?42:0;}";
    square(s3, 42);
}

#[test]
fn a_logical_instruction_takes_no_extended_operand() {
    // DDI 0487 C6.2: only ADD/SUB take an extended register. `orr x0, x1, w3,
    // uxtw` is not an instruction, and the munch table has to know the
    // difference (torture bswap-1, cbrt, and thirteen others).
    let src = "unsigned long f(unsigned long a,unsigned b){return a|b;}\
               int main(void){return f(0x100,0x2a)==0x12a?42:0;}";
    square(src, 42);
    let s2 = "long f(long a,int b){return (a&b)|(a^b);}\
              int main(void){return f(6,3)==7?42:0;}";
    square(s2, 42);
}

// ── sink ───────────────────────────────────────────────────────────────────

#[test]
fn sink_moves_a_computation_to_the_block_that_needs_it() {
    // Not an instruction-count win: it removes nothing. It shortens a LIVE
    // RANGE, which is what §13b measured as the largest remaining item.
    let src = "int f(int a,int b,int c){int t=a*b;if(c)return t+1;return c;}\
               int main(void){return f(6,7,1)-1;}";
    let after = module(src, true);
    let f = func(&after, "f");
    let entry = f.entry as usize;
    assert_eq!(
        f.blocks[entry]
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Bin { op: BinOp::Mul, .. }))
            .count(),
        0,
        "the product is needed on one path only and must move there"
    );
    square(src, 42);
}

#[test]
fn sink_does_not_move_a_computation_into_a_loop() {
    // Sinking into a deeper loop would execute it every iteration — the exact
    // inverse of licm, and the reason the depth is checked.
    let src = "int f(int a,int b,int n){int t=a*b;int i,s=0;for(i=0;i<n;i++)s+=t;return s;}\
               int main(void){return f(6,7,1);}";
    let after = module(src, true);
    let f = func(&after, "f");
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    let in_loop = lf.loops.iter().any(|l| {
        l.body.iter().any(|&b| {
            f.blocks[b as usize]
                .insts
                .iter()
                .any(|i| matches!(i, Inst::Bin { op: BinOp::Mul, .. }))
        })
    });
    assert!(!in_loop, "the product must stay outside the loop");
    square(src, 42);
}

#[test]
fn sink_keeps_a_faulting_computation_where_it_was() {
    // Moving a fault ONTO a path is illegal; the divisor is not a known non-zero
    // literal, so the division stays where the program put it. (The square is
    // not the check here: the ORIGINAL program divides by zero, which is ⊥, and
    // any refinement of ⊥ is legal — so only the structure can say whether the
    // pass obeyed its own rule.)
    let src = "int f(int a,int b,int c){int t=a/b;if(c)return t;return 42;}\
               int main(void){return f(1,2,1);}";
    let after = module(src, true);
    let f = func(&after, "f");
    let entry = f.entry as usize;
    assert_eq!(
        f.blocks[entry]
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. }))
            .count(),
        1,
        "a division that may fault stays on the path that already ran it"
    );
    square(src, 0);
    // a literal non-zero divisor cannot fault, so that one does move
    let ok = "int f(int a,int c){int t=a/2;if(c)return t;return 42;}\
              int main(void){return f(84,1);}";
    let m2 = module(ok, true);
    let g = func(&m2, "f");
    assert_eq!(
        g.blocks[g.entry as usize]
            .insts
            .iter()
            .filter(|i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. }))
            .count(),
        0
    );
    square(ok, 42);
}

// ── the invariant pure-call hoist (REARCH §13c row 1) ──────────────────────

/// Calls left anywhere inside a loop body — the structure the hoist removes.
fn calls_in_loops(f: &Func) -> usize {
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    lf.loops
        .iter()
        .flat_map(|l| l.body.iter())
        .map(|&b| {
            f.blocks[b as usize]
                .insts
                .iter()
                .filter(|i| matches!(i, Inst::Call { .. }))
                .count()
        })
        .sum()
}

/// The summing helper every case below calls: read-only (it stores nothing) and
/// big enough that the inliner leaves it alone, so what the batteries observe is
/// the hoist and not β-reduction.
/// The callee these fences are about. It carries a SECOND call site (`gg`,
/// never called) on purpose: `-finline-functions-called-once` inlines a callee
/// with exactly one call site whatever its linkage (R4.14 (3)), and an inlined
/// callee has no CALL for `calls_in_loops` to count — the fence would then read
/// green for the wrong reason. Two sites keep the call observable, which is what
/// these batteries measure.
const SUM: &str = "int g(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
                   int gg(int *p){return g(p,1);}";

#[test]
fn an_invariant_pure_call_leaves_the_loop() {
    let src = format!(
        "{}int main(void){{int a[4];a[0]=1;a[1]=2;a[2]=3;a[3]=4;\
         int k,s=0;for(k=0;k<10;k++)s+=g(a,4);return s;}}",
        SUM
    );
    let after = module(&src, true);
    assert_eq!(
        calls_in_loops(func(&after, "main")),
        0,
        "g(a,4) reads memory nothing in the loop writes, so it is computed once"
    );
    square(&src, 100);
}

#[test]
fn a_pure_call_is_not_hoisted_across_a_store() {
    // The memory-clean fence. `g` is still read-only, and its ARGUMENTS are
    // still invariant — but the loop writes the array it reads, so the call is a
    // different function of state on every iteration.
    let src = format!(
        "{}int main(void){{int a[2];a[0]=1;a[1]=2;int k,s=0;\
         for(k=0;k<5;k++){{s+=g(a,2);a[0]=a[0]+1;}}return s;}}",
        SUM
    );
    let after = module(&src, true);
    assert_eq!(
        calls_in_loops(func(&after, "main")),
        1,
        "a store in the loop kills the hoist"
    );
    square(&src, 25);
}

#[test]
fn an_impure_call_is_not_hoisted() {
    // The purity fence. Hoisting this would run the counter's increment once
    // instead of five times — a visible change, not an optimization.
    let src = "static int c;int g(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];c=c+1;return s;}\
               int gg(int *p){return g(p,1);}\
               int main(void){int a[2];a[0]=1;a[1]=2;int k,s=0;\
               for(k=0;k<5;k++)s+=g(a,2);return s+c;}";
    let after = module(src, true);
    assert_eq!(
        calls_in_loops(func(&after, "main")),
        1,
        "g writes a global, so it is not read-only"
    );
    square(src, 20);
}

/// The ladder with ROTATION suppressed. Rotation legitimately REMOVES the
/// condition the ≥1-trip fence refuses on — a rotated loop reaches its preheader
/// only through the guard, so it has already run once — which is exactly what
/// §13c predicted and what makes the fence invisible through `module(src, true)`.
/// The fence still has to be proven, so it is proven on the shape it was written
/// against.
fn unrotated(src: &str) -> Module {
    let ast = frontend(src);
    let mut m = build::build(&ast);
    let ro = super::purity::readonly_functions(&m);
    for f in m.funcs.iter_mut() {
        super::super::dom::split_critical_edges(f);
        for _ in 0..super::ROUNDS {
            super::cfg::run(f);
            super::sroa::run(f);
            super::sccp::run(f);
            super::fold::canon(f);
            super::gvn::run(f);
            super::mem::run(f);
            super::licm::run_with(f, &ro);
            super::dce::run(f);
        }
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    m
}

#[test]
fn a_pure_call_is_not_speculated_into_a_loop_that_may_not_run() {
    // The ≥1-trip fence. The bound is a parameter, so the entry test is not
    // decidable, and running `g` in the preheader would call it on a path the
    // program never takes — where it may fault or never return.
    let src = format!(
        "{}int h(int *a,int m){{int k,s=0;for(k=0;k<m;k++)s+=g(a,2);return s;}}\
         int main(void){{int a[2];a[0]=1;a[1]=2;return h(a,0)+42;}}",
        SUM
    );
    assert_eq!(
        calls_in_loops(func(&unrotated(&src), "h")),
        1,
        "an unknown trip count refuses the hoist"
    );
    square(&src, 42);
}

#[test]
fn rotation_licences_the_hoist_the_trip_count_fence_refused() {
    // The other side of the same coin, and the reason rotation was sequenced
    // before this row. Once the loop is rotated its preheader sits UNDER the
    // guard, so reaching it already means the body runs — and the call may be
    // computed there. The square is the proof that this is not wishful: `m` is
    // 0, so the guard fails, the preheader is never entered, and the hoisted
    // call is never made.
    let src = format!(
        "{}int h(int *a,int m){{int k,s=0;for(k=0;k<m;k++)s+=g(a,2);return s;}}\
         int main(void){{int a[2];a[0]=1;a[1]=2;return h(a,0)+42;}}",
        SUM
    );
    assert_eq!(
        calls_in_loops(func(&module(&src, true), "h")),
        0,
        "under the guard the call is loop-invariant and runs once"
    );
    square(&src, 42);
}

#[test]
fn a_conditional_pure_call_is_not_hoisted() {
    // The guaranteed-execution fence. The call's block does not dominate the
    // latch, so the first iteration can close without it.
    let src = format!(
        "{}int h(int *a,int m){{int k,s=0;for(k=0;k<10;k++){{if(k>m)s+=g(a,2);}}return s;}}\
         int main(void){{int a[2];a[0]=1;a[1]=2;return h(a,4);}}",
        SUM
    );
    let after = module(&src, true);
    assert_eq!(
        calls_in_loops(func(&after, "h")),
        1,
        "a call under a condition stays under it"
    );
    square(&src, 15);
}

#[test]
fn purity_survives_recursion() {
    // The fixpoint starts optimistic, which is what makes a read-only RECURSIVE
    // function read-only: "performs a write" is an existential over the body, so
    // a cycle with no writing instruction in it never writes.
    let src = "int g(int *p,int n){if(n<=0)return 0;return p[n-1]+g(p,n-1);}\
               int main(void){int a[3];a[0]=1;a[1]=2;a[2]=3;int k,s=0;\
               for(k=0;k<7;k++)s+=g(a,3);return s;}";
    let after = module(src, true);
    assert_eq!(
        calls_in_loops(func(&after, "main")),
        0,
        "a recursive read-only callee is still read-only"
    );
    square(src, 42);
}

#[test]
fn a_break_after_the_call_still_lets_it_out() {
    // The exit-dominance fence, on its permissive side. The loop leaves through
    // a `break` that only the call's own block can reach, so an iteration that
    // breaks has already made the call — and computing it in the preheader adds
    // no execution.
    let src = format!(
        "{}int h(int *a,int m){{int k,s=0;for(k=0;k<10;k++){{s+=g(a,2);if(s>m)break;}}return s;}}\
         int main(void){{int a[2];a[0]=1;a[1]=2;return h(a,7);}}",
        SUM
    );
    let after = module(&src, true);
    assert_eq!(
        calls_in_loops(func(&after, "h")),
        0,
        "an exit the call dominates does not block the hoist"
    );
    square(&src, 9);
}

#[test]
fn a_break_before_the_call_keeps_it_in() {
    // The same fence, on its refusing side: this loop can leave BEFORE ever
    // reaching the call, so hoisting would speculate it onto a path the program
    // never takes. The difference from the case above is the order, and the
    // dominance test is what sees it.
    let src = format!(
        "{}int h(int *a,int m){{int k,s=0;for(k=0;k<10;k++){{if(k>m)break;s+=g(a,2);}}return s;}}\
         int main(void){{int a[2];a[0]=1;a[1]=2;return h(a,3);}}",
        SUM
    );
    let after = module(&src, true);
    assert_eq!(
        calls_in_loops(func(&after, "h")),
        1,
        "an exit ahead of the call keeps it inside"
    );
    square(&src, 12);
}

// ── loop rotation (REARCH §13c row 2, shipped default-OFF — see §13e) ──────

/// Build, promote locals, then rotate by hand. `module(src, true)` cannot reach
/// this pass because it ships disabled, and a disabled theorem still owes its
/// square — otherwise enabling it later would enable something unproven.
fn rotated(src: &str) -> (Module, bool) {
    let ast = frontend(src);
    let mut m = build::build(&ast);
    let mut any = false;
    for f in m.funcs.iter_mut() {
        super::super::dom::split_critical_edges(f);
        for _ in 0..super::ROUNDS {
            super::cfg::run(f);
            super::sroa::run(f);
            super::sccp::run(f);
            super::gvn::run(f);
        }
        any |= super::rotate::force(f);
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    (m, any)
}

/// ⟦f⟧ = ⟦rotate f⟧, on the interpreter, both sides equal `want`.
fn rotate_square(src: &str, want: i64) {
    let ast = frontend(src);
    let plain = build::build(&ast);
    let (rot, _) = rotated(src);
    match (run(&plain, &ast), run(&rot, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == want => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦rotate f⟧={:?} want {}\n{}", x, y, want, src),
    }
}

/// Does the loop leave from its HEADER? That is what "top-tested" means, and its
/// disappearance is the structural consequence rotation exists to produce. Asked
/// of the header rather than of the latch because rotation ends by splitting the
/// critical edges it created, and the latch is then a split block that merely
/// forwards — a true statement about layout, but not the one under test.
fn header_is_the_test(f: &Func) -> bool {
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    lf.loops.iter().any(|l| {
        f.blocks[l.header as usize]
            .term
            .succs()
            .iter()
            .any(|s| !l.body.contains(s))
    })
}

#[test]
fn rotation_moves_the_test_to_the_bottom() {
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    let (before, _) = (build::build(&frontend(src)), 0);
    assert!(
        header_is_the_test(func(&before, "f")),
        "the frontend builds a top-tested loop — otherwise this proves nothing"
    );
    let (after, fired) = rotated(src);
    assert!(fired, "a counted loop must rotate");
    assert!(
        !header_is_the_test(func(&after, "f")),
        "after rotation the loop no longer leaves from its header"
    );
    rotate_square(src, 42);
}

#[test]
fn rotation_preserves_a_loop_that_never_runs() {
    // The whole content of the square: the guard is the header's FIRST
    // execution, so a loop entered zero times still evaluates the test once and
    // takes the same exit.
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[1];a[0]=7;return f(a,0)+42;}";
    rotate_square(src, 42);
}

#[test]
fn rotation_refuses_a_header_that_stores() {
    // The header is COPIED. A read may be copied — each dynamic execution still
    // happens once — but a store would stand at two program points.
    let src = "int f(int *p,int n){int i=0;while(p[i]++ < n) i++; return i;}\
               int main(void){int a[3];a[0]=0;a[1]=1;a[2]=9;return f(a,2)+40;}";
    let (after, fired) = rotated(src);
    assert!(!fired, "a header with a store is not copyable");
    assert!(header_is_the_test(func(&after, "f")), "so the loop stays top-tested");
}

#[test]
fn rotation_is_not_applied_twice() {
    // Termination. A rotated loop tests on its latch, and that is exactly the
    // shape the pass refuses, so the peel cannot chain.
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    let (mut m, fired) = rotated(src);
    assert!(fired);
    for f in m.funcs.iter_mut() {
        assert!(!super::rotate::force(f), "rotation must be idempotent");
    }
}

// ── scalar evolution (REARCH §13f — the prerequisite for pointer-IV / LFTR) ──

/// The optimized module plus the scev of its first loop in `name`.
fn scev_of(src: &str, name: &str) -> (Module, Option<usize>) {
    let m = module(src, true);
    let f = func(&m, name);
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    let li = (0..lf.loops.len()).find(|&i| {
        super::scev::LoopScev::analyze(f, &c, &dt, &lf, i).is_some()
    });
    (m.clone(), li)
}

/// Run `k` over the scev of the first analysable loop of `name`.
fn with_scev<R>(src: &str, name: &str, k: impl Fn(&Func, &super::scev::LoopScev, &super::dom::Cfg) -> R) -> R {
    let (m, li) = scev_of(src, name);
    let f = func(&m, name);
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    let li = li.expect("no analysable loop");
    let s = super::scev::LoopScev::analyze(f, &c, &dt, &lf, li).unwrap();
    k(f, &s, &c)
}

#[test]
fn scev_finds_the_counter_of_a_counted_loop() {
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    with_scev(&src, "f", |_f, s, _c| {
        let iv = s.ivs.values().find(|a| a.step == 1).expect("a +1 counter");
        assert_eq!(iv.base, None, "the counter starts at a literal");
        assert_eq!(iv.off, 0, "and that literal is 0");
    });
}

#[test]
fn scev_reads_an_address_as_base_plus_stride() {
    // The shape pointer-IV exists for: `p[i]` is `p + sext(i)*4`, an affine
    // recurrence over the invariant base `p` with the element size as its step.
    // The bound is a LITERAL, and it has to be: seeing through the widening is
    // what needs the trip count (`stays_in_range`).
    let src = "int f(int *p){int i,s=0;for(i=0;i<3;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a);}";
    with_scev(&src, "f", |f, s, _c| {
        let mut strides: Vec<i64> = Vec::new();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Load { addr, .. } = inst {
                    if let Some(a) = s.eval(f, *addr) {
                        if a.step != 0 {
                            assert!(a.base.is_some(), "the address is based on the pointer");
                            strides.push(a.step);
                        }
                    }
                }
            }
        }
        assert_eq!(strides, vec![4], "one load, striding by sizeof(int)");
    });
}

#[test]
fn scev_bounds_the_widening_by_the_exit_test() {
    // The same program with the bound as a PARAMETER, so there is no trip count
    // — and the recurrence is found anyway. Inside the body the test has passed,
    // so `i < n`; `n` IS an `int`, so `n <= INT_MAX`; so the counter cannot leave
    // its type. Nothing about the VALUE of `n` is used, only its TYPE, which is
    // why this works where a trip count cannot. This is the case every hot loop
    // in the suite is, and without it pointer-IV would fire only on literal
    // bounds.
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    with_scev(&src, "f", |f, s, _c| {
        assert_eq!(s.trips, None, "an unknown bound still has no trip count");
        let strided = f.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, Inst::Load { addr, .. }
                if s.eval(f, *addr).map_or(false, |a| a.step == 4 && a.base.is_some()))
        });
        assert!(strided, "the exit test bounds the counter, so the address is affine");
    });
}

#[test]
fn scev_refuses_a_widening_the_test_cannot_bound() {
    // `i += 2` against a symbolic bound. The argument above needs the step to be
    // exactly one: at `n == INT_MAX` this counter runs ..., INT_MAX-1, and the
    // next increment OVERFLOWS — which SEMANTICS §7 makes a defined, wrapping
    // execution, not undefined behaviour to assume away. The test would then see
    // a negative value, say "stay in", and the loop would walk off with an
    // address a affine recurrence had promised. So it is refused.
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i+=2)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    with_scev(&src, "f", |f, s, _c| {
        let strided = f.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, Inst::Load { addr, .. }
                if s.eval(f, *addr).map_or(false, |a| a.step != 0))
        });
        assert!(!strided, "a step of two cannot be proven not to overflow");
    });
}

#[test]
fn scev_counts_the_trips_of_a_literal_loop() {
    let src = "int f(void){int i,s=0;for(i=0;i<10;i++)s+=i;return s;}\
               int main(void){return f();}";
    with_scev(&src, "f", |_f, s, c| {
        let _ = c; assert_eq!(s.trips, Some(10));
    });
}

#[test]
fn scev_counts_a_stride_that_does_not_divide_the_range() {
    // 0,3,6,9 — four trips, not three: `ceil((10-0)/3)`. An off-by-one here is
    // written straight into the program by final-value, so it is pinned.
    let src = "int f(void){int i,s=0;for(i=0;i<10;i+=3)s+=i;return s;}\
               int main(void){return f();}";
    with_scev(&src, "f", |_f, s, c| {
        let _ = c; assert_eq!(s.trips, Some(4));
    });
}

#[test]
fn scev_refuses_a_bound_it_cannot_see() {
    // The bound is a parameter, so there is no exact count — and `None` is the
    // answer, never a guess.
    let src = "int f(int n){int i,s=0;for(i=0;i<n;i++)s+=i;return s;}\
               int main(void){return f(9);}";
    with_scev(&src, "f", |_f, s, c| {
        let _ = c; assert_eq!(s.trips, None);
    });
}

#[test]
fn scev_refuses_a_value_outside_the_affine_fragment() {
    // A counter multiplied by ITSELF is quadratic; the analysis has no form for
    // it and must say so rather than linearize it.
    let src = "int f(int n){int i,s=0;for(i=0;i<n;i++)s+=i*i;return s;}\
               int main(void){return f(4);}";
    with_scev(&src, "f", |f, s, _c| {
        let quad = f.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, Inst::Bin { op: BinOp::Mul, dst, .. }
                if s.eval(f, Operand::Val(*dst)).is_none())
        });
        assert!(quad, "i*i is not affine and must not evaluate");
    });
}

// ── pointer induction variables (§13h). The UNIT-STRIDE half ships OFF (§13i,
// MEASURED M2); the row-strided half ships ON (§13q) ──────────────────────

/// The optimized module, then this pass by hand — `module(src, true)` cannot
/// reach it while it ships disabled, and a disabled theorem still owes its
/// square.
fn with_iv(src: &str) -> (Module, bool) {
    let ast = frontend(src);
    let mut m = build::build(&ast);
    super::run_module_with(&mut m, &crate::compile::pinned_symbols(&ast));
    let mut any = false;
    for f in m.funcs.iter_mut() {
        any |= super::iv::force(f);
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    (m, any)
}

/// Loads whose address is a block PARAMETER — a pointer being walked, rather
/// than an address rebuilt from a counter.
fn walked_loads(f: &Func) -> usize {
    let params: std::collections::HashSet<u32> =
        f.blocks.iter().flat_map(|b| b.params.iter().copied()).collect();
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, Inst::Load { addr: Operand::Val(v), .. } if params.contains(v)))
        .count()
}

#[test]
fn a_strided_load_walks_a_pointer() {
    let src = "int f(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}\
               int main(void){int a[3];a[0]=20;a[1]=15;a[2]=7;return f(a,3);}";
    let (before, _) = (build::build(&frontend(src)), 0);
    let _ = &before;
    let (after, fired) = with_iv(src);
    assert!(fired, "an affine load address must be strength-reduced");
    assert_eq!(walked_loads(func(&after, "f")), 1, "p[i] becomes a walking pointer");
    // ⟦f⟧ = ⟦iv f⟧, both equal what C99 says
    let plain = {
        let ast = frontend(src);
        let mut m = build::build(&ast);
        super::run_module_with(&mut m, &crate::compile::pinned_symbols(&ast));
        m
    };
    let ast = frontend(src);
    match (run(&plain, &ast), run(&after, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 42 => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦iv f⟧={:?} want 42", x, y),
    }
}

#[test]
fn a_row_strided_load_walks_a_pointer_by_default() {
    // The half of the pass that is ON, and the reason the two halves are told
    // apart. `m[k][j]` walks a ROW: step 40 bytes for an 8-byte access. A64's
    // scaled index scales by the ACCESS SIZE and by nothing else (DDI 0487
    // C6.2.130), and 40 is not a power of two either, so no shift reaches it —
    // the address is rebuilt with a MULTIPLY on every iteration. Replacing that
    // multiply with an `add` is the same instruction count and one multiply less
    // in front of a load, which is why it needs no post-index to pay and does
    // not share MEASURED M2's verdict. FIVE columns deliberately: a row stride
    // of 32 or 64 is a `lsl`, and a shift is not the multiply this removes.
    //
    // The address is also `&m + k*40 + j*8`: TWO loop-invariant symbolic terms
    // around one recurrence, which `scev::AddRec` alone cannot hold — so this is
    // the non-vacuity proof for `affine`'s split as well.
    let src = "long m[8][5];\
               long f(int j){long s=0;int k;for(k=0;k<8;k++)s+=m[k][j];return s;}\
               int main(void){int i,j;for(i=0;i<8;i++)for(j=0;j<5;j++)m[i][j]=i*5+j;\
               return (int)f(3);}";
    let opt = module(src, true);
    assert_eq!(
        walked_loads(func(&opt, "f")),
        1,
        "a row-strided load must walk a pointer with the pass at its DEFAULT setting"
    );
    // ⟦f⟧ = ⟦iv f⟧, and both equal what C99 says: m[k][3] = k*5+3 for k = 0..7,
    // so the sum is 5*(0+1+…+7) + 8*3 = 140 + 24 = 164.
    let ast = frontend(src);
    let plain = module(src, false);
    match (run(&plain, &ast), run(&opt, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 164 => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦iv f⟧={:?} want 164", x, y),
    }
}

#[test]
fn a_stored_address_keeps_its_scaled_index() {
    // Loads only, and it is a COST rule: A64 reaches `p[i]` with a free scaled
    // index, so replacing it pays only when the increment then vanishes into a
    // post-index — which A64 offers for loads alone. A store-only walk was
    // measured a LOSS (j2_histogram's zeroing loop, 60 → 69 ms).
    let src = "void f(int *p,int n){int i;for(i=0;i<n;i++)p[i]=i;}\
               int main(void){int a[3];f(a,3);return a[0]+a[1]*2+a[2]*20;}";
    let (after, fired) = with_iv(src);
    assert!(!fired, "a store address is left to the addressing mode");
    assert_eq!(walked_loads(func(&after, "f")), 0);
}

// ── R4.5 / R4.9: booleans stay flags, and memory crosses one edge ──────────

#[test]
fn a_short_circuit_condition_reaches_the_branch_as_flags() {
    // §13n row (d): `a && b` builds a VALUE — one arm computes a relation, the
    // other passes 0 — and the merge is then branched on. sqlite paid 3,707
    // pure-boolean `csel` and 669 `csel → cbnz` for it against gcc's 9.
    // Identity (e): the arm that passes a LITERAL already knows which way the
    // merge's branch goes, so it names the destination directly; identity (d)
    // then folds what is left into the block that computed the relation, and
    // isel fuses the compare into the branch.
    let src = "int f(int*p,int n,int k){int i=0;while(i<n&&p[i]!=k)i++;return i;}\
               int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i;return f(a,4,2);}";
    square(src, 2);
    let m = module(src, true);
    let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
    // no boolean is materialized: the `&&` is two branches, not a value
    assert_eq!(count(f, |i| matches!(i, Inst::Select { .. })), 0, "a boolean survived");
    // and the two relations still exist — the pass threaded, it did not delete
    assert!(count(f, |i| matches!(i, Inst::Cmp { .. })) >= 2);
}

#[test]
fn threading_refuses_a_block_whose_parameter_is_read_below_it() {
    // THE SIDE CONDITION. Skipping a block skips the DEFINITION of its
    // parameters, and SSA licences a use anywhere that block dominates —
    // arbitrarily far below the immediate successor the substitution reaches.
    // A loop header whose induction parameter the body reads directly is
    // exactly that shape, and without the condition `hir::verify` reports
    // `%24 used in bb6 but defined in bb2` (torture pr54937/pr109925/pr116799,
    // and sqlite `unixLock`). The battery's obligation here is that the pass
    // REFUSES: `module` verifies every function, so a bad thread is a panic.
    square(
        "void g(int);\
         void t(int c){int i;for(i=0;i<c;i++){if(i)g(i);}}\
         int s;void g(int x){s+=x;}\
         int main(void){t(5);return s;}",
        10,
    );
    square(
        "int f(int n,int m){int i,r=0;for(i=0;i<n;i++){if(i&1)r+=i;else r+=m;}return r;}\
         int main(void){return f(6,10);}",
        39,
    );
}

#[test]
fn a_load_survives_one_edge_into_a_single_predecessor_block() {
    // R4.9 (§13n row (h), gcc's `-ftree-fre`). A block whose ONLY predecessor
    // is P is entered exactly once per execution of P, immediately after it —
    // so the memory state at its entry IS P's exit state, and a load P already
    // performed need not be repeated. j5_insertion_sort loads `p[j]` in the
    // loop's condition and again in the body, which has that condition block as
    // its only predecessor.
    let src = "void s(int*p,int n){int i,j;for(i=1;i<n;i++){int k=p[i];j=i-1;\
               while(j>=0&&p[j]>k){p[j+1]=p[j];j--;}p[j+1]=k;}}\
               int main(void){int a[5];int i;for(i=0;i<5;i++)a[i]=5-i;\
               s(a,5);return a[0]*10000+a[1]*1000+a[2]*100+a[3]*10+a[4];}";
    square(src, 12345);
    let m = module(src, true);
    let f = m.funcs.iter().find(|f| f.name == "s").unwrap();
    let loads = count(f, |i| matches!(i, Inst::Load { .. }));
    // the condition's `p[j]` and the body's `p[j]` are one load, not two
    assert!(loads <= 2, "the body re-loaded what the condition already read ({} loads)", loads);
    // …and a store between two loads still forces the second
    square(
        "void w(int*p){p[0]=p[1];p[1]=p[0]+p[1];}\
         int main(void){int a[2];a[0]=3;a[1]=7;w(a);return a[0]*10+a[1];}",
        84,
    );
}

// ── R4.11 / R4.14: rotation over every exit, and the exact reciprocal ──────

#[test]
fn a_loop_with_an_early_return_rotates() {
    // R4.11. Rotation used to demand a SINGLE door: one exit block, with one
    // predecessor, dominating every reader of a header value. The residual print
    // measured what that costs on sqlite — 1,837 loops refused for "the exit
    // block is a merge" and 221 for "outside the exit's dominance", the two
    // largest fixable reasons. Both shapes are ordinary C: a loop with an early
    // `return` has two exits, and `while (a && b)` reaches one exit from two
    // different in-loop blocks.
    let src = "int find(int*p,int n,int k){int i;for(i=0;i<n;i++)if(p[i]==k)return i;return -1;}\
               int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i*3;\
               return find(a,4,6)*10+find(a,4,5)+11;}";
    square(src, 30);
    let m = module(src, true);
    let f = func(&m, "find");
    // Bottom-tested: the block that exits the loop is one of its latches, so
    // the back edge IS the test and no unconditional branch returns to a header.
    let c = super::dom::cfg(f);
    let dt = super::dom::domtree(f, &c);
    let lf = super::dom::loops(&c, &dt);
    assert_eq!(lf.loops.len(), 1, "one loop");
    let l = &lf.loops[0];
    let inl = |b: BlockId| l.body.contains(&b);
    let exiting: Vec<BlockId> = l
        .body
        .iter()
        .copied()
        .filter(|&b| f.blocks[b as usize].term.succs().iter().any(|&s| !inl(s)))
        .collect();
    assert!(
        exiting.iter().any(|e| l.latches.contains(e)),
        "the loop still tests at the top: exiting {:?} latches {:?}",
        exiting,
        l.latches
    );
    // …and the two-exit shape keeps working when a header value leaves through
    // BOTH doors.
    square(
        "int f(int*p,int n){int i,s=0;for(i=0;i<n;i++){s+=p[i];if(s>10)return i;}return s;}\
         int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i*4;return f(a,4)+f(a,2);}",
        6,
    );
}

#[test]
fn a_division_by_a_power_of_two_becomes_a_multiplication() {
    // R4.14 (1). Exact under IEEE 754: a power of two has an all-zero
    // significand, so its reciprocal is representable, and both operations are
    // the correctly-rounded result of the same exact real number. `fdiv` is 10+
    // cycles on this machine and `fmul` is 3.
    let src = "double f(double x){return x/1024.0;} \
               int main(void){return (int)(f(4096.0)*2.0);}";
    square(src, 8);
    let m = module(src, true);
    let f = func(&m, "f");
    assert_eq!(count(f, |i| matches!(i, Inst::Bin { op: BinOp::FDiv, .. })), 0);
    assert_eq!(count(f, |i| matches!(i, Inst::Bin { op: BinOp::FMul, .. })), 1);
    // a divisor that is NOT a power of two keeps its division
    let g = module("double f(double x){return x/10.0;} int main(void){return (int)f(100.0);}", true);
    let g = func(&g, "f");
    assert_eq!(count(g, |i| matches!(i, Inst::Bin { op: BinOp::FDiv, .. })), 1);
    // …and so does one whose reciprocal is not representable
    square("double f(double x){return x/1e300;} int main(void){return f(1e300)==1.0;}", 1);
    square("float f(float x){return x/8.0f;} int main(void){return (int)f(64.0f);}", 8);
    square("double f(double x){return x/-2.0;} int main(void){return (int)f(-8.0);}", 4);
}

#[test]
fn an_invariant_plus_the_counter_becomes_the_counter() {
    // §13q ii / Law 3c. `(m + k) & 31` inside a `k` loop rebuilds `m + k` on
    // every iteration; gcc runs that value AS the counter and shifts the bound
    // by `m`. Six instructions become five, and d2_nested_loops 1.400 → 1.000.
    let src = "long f(int n,int m){long s=0;int k;for(k=0;k<n;k++)s+=(m+k)&31;return s;}\
               int main(void){return (int)f(5,7);}";
    let opt = module(src, true);
    let g = func(&opt, "f");
    // NON-VACUITY, and it is the whole point: the narrow counter is GONE. Both
    // I32 adds — the step and the `m + k` — went with it, and the exit test is
    // now made at 64 bits.
    assert_eq!(count(g, |i| matches!(i, Inst::Bin { op: BinOp::Add, ty: crate::hir::Ty::I32, .. })), 0);
    // The guard rotation left in the entry block is still an I32 compare and
    // legitimately so — it is outside the loop. What matters is that the LOOP's
    // test is now made at 64 bits.
    assert_eq!(count(g, |i| matches!(i, Inst::Cmp { ty: crate::hir::Ty::I64, .. })), 1);
    assert!(
        g.blocks.iter().any(|b| b.params.iter().any(|&p| g.ty_of(p) == crate::hir::Ty::I64)),
        "the substituted counter is a wide header parameter"
    );
    // ⟦f⟧ = ⟦subst f⟧, both equal C99: (7+k)&31 for k = 0..4 is 7+8+9+10+11 = 45.
    let ast = frontend(src);
    let plain = module(src, false);
    match (run(&plain, &ast), run(&opt, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 45 => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦subst f⟧={:?} want 45", x, y),
    }
}

#[test]
fn a_masked_truncation_needs_no_widening() {
    // `fold::narrow_mask`'s square, on its own. A mask that clears the sign bit
    // makes both the truncation and the widening around it no-ops, so the whole
    // sandwich is one 64-bit `and`. Without it `iv::substitute` hands back the
    // instruction it saved as a `mov w,w`.
    let src = "long g(long x){return (long)(((int)x)&31);}\
               int main(void){return (int)g(100);}";
    let opt = module(src, true);
    let g = func(&opt, "g");
    assert_eq!(count(g, |i| matches!(i, Inst::Cvt { .. })), 0, "no trunc, no widening");
    assert_eq!(count(g, |i| matches!(i, Inst::Bin { op: BinOp::And, ty: crate::hir::Ty::I64, .. })), 1);
    let ast = frontend(src);
    let plain = module(src, false);
    match (run(&plain, &ast), run(&opt, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 4 => {}
        (x, y) => panic!("⟦g⟧={:?} ⟦narrow_mask g⟧={:?} want 4", x, y),
    }
    // The mask must clear the SIGN bit; 0xffffffff does not, and the rule must
    // refuse it rather than turn a negative into a positive.
    let neg = "long g(long x){return (long)(((int)x)&-1);}\
               int main(void){return (int)g(-7);}";
    let ast = frontend(neg);
    match (run(&module(neg, false), &ast), run(&module(neg, true), &ast)) {
        (Ok(x), Ok(y)) if x == y && x == -7 => {}
        (x, y) => panic!("a sign-bit-keeping mask must be refused: {:?} {:?}", x, y),
    }
}

#[test]
fn a_counted_loop_counts_down_and_the_compare_disappears() {
    // h2_revbits' shape: an index read by nothing but its own step and the exit
    // test, so its VALUES are unobservable and only the trip count matters.
    // Counting down lets the step set the flags the branch wants, and the
    // separate `cmp` against the bound goes away — six instructions to five,
    // 1.194 to 1.000 on the clock.
    let src = "unsigned revbits(unsigned x){unsigned r=0;int i;\
               for(i=0;i<32;i++){r=(r<<1)|(x&1);x>>=1;}return r;}\
               int main(void){return (int)(revbits(1u)>>28);}";
    let opt = module(src, true);
    let g = func(&opt, "revbits");
    // NON-VACUITY: the test is against ZERO now, not against the bound.
    assert_eq!(
        count(g, |i| matches!(
            i,
            Inst::Cmp { op: crate::hir::CmpOp::Ne, b: Operand::Imm(0), .. }
        )),
        1,
        "the exit test counts down to zero"
    );
    // ⟦f⟧ = ⟦countdown f⟧. revbits(1) reverses bit 0 into bit 31, so the value
    // is 0x80000000 and its top nibble is 8.
    let ast = frontend(src);
    let plain = module(src, false);
    match (run(&plain, &ast), run(&opt, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 8 => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦countdown f⟧={:?} want 8", x, y),
    }
}

#[test]
fn counting_down_is_idempotent_under_the_fixpoint() {
    // THE REGRESSION THIS EXISTS FOR. The ladder re-runs every pass until
    // nothing changes, so a rewrite that cannot recognize its OWN OUTPUT runs
    // again on it. `countdown`'s first cut guarded on the trip count rather than
    // on the shape of the test; on the second round `scev` re-derived a count of
    // 1 from the rewritten `!= 0` test, the loop was rebuilt to start at 1, and
    // `revbits` ran a single iteration — 6442439334100992 became 1500000.
    //
    // Running the whole ladder twice must therefore be the same as running it
    // once, and the answer must still be the answer.
    let src = "unsigned revbits(unsigned x){unsigned r=0;int i;\
               for(i=0;i<32;i++){r=(r<<1)|(x&1);x>>=1;}return r;}\
               int main(void){return (int)(revbits(1u)>>28);}";
    let ast = frontend(src);
    let mut once = build::build(&ast);
    super::run_module_with(&mut once, &crate::compile::pinned_symbols(&ast));
    let mut twice = build::build(&ast);
    super::run_module_with(&mut twice, &crate::compile::pinned_symbols(&ast));
    super::run_module_with(&mut twice, &crate::compile::pinned_symbols(&ast));
    for f in &twice.funcs {
        verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    assert_eq!(
        ninsts(func(&once, "revbits")),
        ninsts(func(&twice, "revbits")),
        "a second run of the ladder must change nothing"
    );
    match (run(&once, &ast), run(&twice, &ast)) {
        (Ok(a), Ok(b)) if a == b && a == 8 => {}
        (a, b) => panic!("once={:?} twice={:?} want 8", a, b),
    }
}

#[test]
fn countdown_refuses_an_index_shared_with_a_sibling_header_phi() {
    // THE REGRESSION THIS EXISTS FOR (`c9330`, found by the 20k csmith seal and
    // reduced to fifteen lines). Taking `&i` keeps the index in memory, so the
    // promoter rebuilds it as TWO header parameters fed by the SAME latch value:
    // one slot read by the exit test, the other by `&a[i]`. `countdown` rewrites
    // the step instruction in place, which turns both slots into countdowns, but
    // restamps the entry argument of only its own slot. The address phi
    // therefore ran 0, 4, 3, 2, 1 and the loop stored into `a[1]` where the
    // source says `a[4]`.
    //
    // The fence is that the step must flow into no header slot but its own, so
    // the pass must simply decline this loop.
    let src = "int a[5];int *p;int **pp;\
               int main(void){int i;pp=&p;\
               for(i=0;i<=4;i+=1) *pp=&a[i];\
               **pp=42;{int *q=&i;(void)q;}\
               return a[4]*10+a[1];}";
    let ast = frontend(src);
    let plain = module(src, false);
    let opt = module(src, true);
    match (run(&plain, &ast), run(&opt, &ast)) {
        (Ok(x), Ok(y)) if x == y && x == 420 => {}
        (x, y) => panic!("⟦f⟧={:?} ⟦countdown f⟧={:?} want 420", x, y),
    }
}

// ── R5.2 TBAA ──────────────────────────────────────────────────────────────

/// The same program built with the alias classes stamped, and without.
fn tbaa_module(src: &str, on: bool) -> Module {
    crate::hir::set_tbaa(Some(on));
    let m = module(src, true);
    crate::hir::set_tbaa(None);
    m
}

fn loads(f: &Func) -> usize {
    count(f, |i| matches!(i, Inst::Load { .. }))
}

/// EFFECT (2). Two pointers of incompatible type cannot name one object (C99
/// 6.5p7), so the store through the `float *` cannot be what the `int *` reads —
/// and the reload after it is redundant. The address oracle alone cannot see
/// this: both locations are `Loc::Ptr` of unrelated values, which is the one
/// case it answers "may alias" to.
#[test]
fn tbaa_disambiguates_two_incompatible_pointers() {
    let src = "int tb(int *p,float *q,int v){*p=v;*q=1.0f;return *p;}\
               int main(void){int a;float b;return tb(&a,&b,7);}";
    let off = loads(func(&tbaa_module(src, false), "tb"));
    let on = loads(func(&tbaa_module(src, true), "tb"));
    assert!(on < off, "the class was stamped but nothing read it: {} -> {}", off, on);
}

/// SOUNDNESS (1), the direction that matters. A CHARACTER type may access any
/// object (6.5p7's last bullet), so a `char *` store must keep killing the
/// available `int` — and `main` here passes the SAME object under both names, so
/// a wrong answer is a wrong answer and not a technicality.
#[test]
fn tbaa_keeps_a_character_access_aliasing_everything() {
    let src = "int cb(int *p,char *q){*p=258;*q=0;return *p;}\
               int main(void){int a=0;return cb(&a,(char*)&a);}";
    let off = loads(func(&tbaa_module(src, false), "cb"));
    let on = loads(func(&tbaa_module(src, true), "cb"));
    assert_eq!(on, off, "a char access stopped aliasing: {} -> {}", off, on);
    crate::hir::set_tbaa(Some(true));
    square(src, 256);
    crate::hir::set_tbaa(None);
}

/// SOUNDNESS (1) — THE UNION. Reading a member other than the one last stored is
/// the idiom C99 softened rather than outlawed (6.5.2.3, TC3 footnote 82), so an
/// access reached through a union member carries ANY however well-typed it looks.
/// Here `u->i` is read after `u->f` overwrote it: disambiguating the two would
/// forward the dead `5` and return it.
#[test]
fn tbaa_refuses_to_disambiguate_through_a_union() {
    let src = "union U{int i;float f;};\
               int pun(union U *u){u->i=5;u->f=2.0f;return u->i;}\
               int main(void){union U u;return pun(&u);}";
    crate::hir::set_tbaa(Some(true));
    // 2.0f is 0x40000000 read back as an int
    square(src, 1073741824);
    crate::hir::set_tbaa(None);
}

// ── R5.5 VRP ───────────────────────────────────────────────────────────────

fn vrp_module(src: &str, on: bool) -> Module {
    super::vrp::set_vrp(Some(on));
    let m = module(src, true);
    super::vrp::set_vrp(None);
    m
}

fn cmps(f: &Func) -> usize {
    count(f, |i| matches!(i, Inst::Cmp { .. }))
}

/// EFFECT (2) — THE GUARD. Inside `if (n < 16)` the test `n < 100` is not a
/// branch: every value the first test admits passes the second. Nothing in the
/// definition of `n` says so — the fact is on the EDGE, which is what the
/// dominator-inherited constraint map carries.
#[test]
fn vrp_folds_a_comparison_the_guard_above_it_decides() {
    let src = "int g(int n){if(n<16){if(n<100)return 1;return 2;}return 3;}\
               int main(void){return g(4)+g(40);}";
    let off = cmps(func(&vrp_module(src, false), "g"));
    let on = cmps(func(&vrp_module(src, true), "g"));
    assert!(on < off, "the guarded comparison survived: {} -> {}", off, on);
    super::vrp::set_vrp(Some(true));
    square(src, 4); // g(4) = 1, g(40) = 3
    super::vrp::set_vrp(None);
}

/// EFFECT (2) — THE MASK. `x & 0xff` is in `[0, 255]` whatever `x` was, so a
/// comparison against a larger bound is decided. This is the arm that needs no
/// control flow at all: the fact comes from the operation.
#[test]
fn vrp_bounds_a_masked_value() {
    let src = "int g(int x){int m=x&255;if(m>300)return 7;return m;}\
               int main(void){return g(1000);}";
    let off = cmps(func(&vrp_module(src, false), "g"));
    let on = cmps(func(&vrp_module(src, true), "g"));
    assert!(on < off, "the mask bound nothing: {} -> {}", off, on);
    super::vrp::set_vrp(Some(true));
    square(src, 232); // 1000 & 255
    super::vrp::set_vrp(None);
}

/// EFFECT (2) — THE DIVISION. `x / 4` on a dividend proven non-negative is a
/// shift; on one that is not, it is the three-instruction rounding dance, and
/// the pass must leave it alone. Both halves are asserted, because a rewrite
/// that fired on the second would be a miscompile and one that fired on neither
/// would be a vacuous green.
#[test]
fn vrp_reduces_a_signed_division_only_where_the_dividend_is_proven_non_negative() {
    let sdiv = |f: &Func| {
        count(f, |i| matches!(i, Inst::Bin { op: BinOp::SDiv, .. } | Inst::Bin { op: BinOp::SRem, .. }))
    };
    let proven = "int g(int x){int m=x&255;return m/4+m%8;}int main(void){return g(1000);}";
    let unproven = "int g(int x){return x/4+x%8;}int main(void){return g(-1000);}";
    assert!(sdiv(func(&vrp_module(proven, false), "g")) > 0, "the fixture has no division");
    assert_eq!(
        sdiv(func(&vrp_module(proven, true), "g")),
        0,
        "a non-negative dividend still divides"
    );
    assert_eq!(
        sdiv(func(&vrp_module(unproven, true), "g")),
        sdiv(func(&vrp_module(unproven, false), "g")),
        "a dividend that may be negative was rewritten: the rounding is not the same"
    );
    super::vrp::set_vrp(Some(true));
    square(proven, 58); // 232/4 = 58, 232%8 = 0
    square(unproven, -250); // -1000/4 = -250, -1000%8 = 0
    super::vrp::set_vrp(None);
}

/// THEORY A7b  SQUARE vrp_replaces_an_expression_by_one_equal_on_its_range
///
/// The obligation itself, on the shapes the pass rewrites and on the shapes it
/// must refuse: a guard that decides a comparison, a mask that bounds one, a
/// division whose dividend is proven non-negative, a division whose dividend is
/// not, a loop whose counter the widening sends to the full width, and the
/// boundary values where an interval argument is easiest to get wrong.
#[test]
fn vrp_replaces_an_expression_by_one_equal_on_its_range() {
    super::vrp::set_vrp(Some(true));
    let cases: &[(&str, i64)] = &[
        ("int g(int n){if(n<16){if(n<100)return 1;return 2;}return 3;}\
          int main(void){return g(4)*10+g(40);}", 13),
        ("int g(int x){int m=x&255;return m/4+m%8;}int main(void){return g(1000);}", 58),
        ("int g(int x){return x/4+x%8;}int main(void){return g(-1000);}", -250),
        // the boundary the interval arithmetic must not round through
        ("int g(int x){int m=x&255;if(m>255)return 1;if(m<0)return 2;return 3;}\
          int main(void){return g(-1);}", 3),
        // INT_MIN / -1 is the one signed division the hardware cannot do; the
        // pass must not claim a range that would let anything rewrite it
        ("int g(int x){return x/2;}int main(void){return g(-2147483647-1)==-1073741824;}", 1),
        // a loop counter: the widening sends it to the full width, so nothing
        // below may assume a bound the program does not prove
        ("int main(void){int i,s=0;for(i=0;i<10;i++){if(i>=0)s+=i;else s-=i;}return s;}", 45),
        // an unsigned comparison against a value that straddles zero stays open
        ("int g(int x){unsigned u=(unsigned)x;if(u<10u)return 1;return 2;}\
          int main(void){return g(-1);}", 2),
    ];
    for (src, want) in cases {
        square(src, *want);
    }
    super::vrp::set_vrp(None);
    // …and the square is not vacuous: on the first case the pass removes a
    // comparison, and on the second it removes the division. A square that only
    // said "still correct" would stay green for a pass that never fired.
    assert!(
        cmps(func(&vrp_module(cases[0].0, true), "g")) < cmps(func(&vrp_module(cases[0].0, false), "g")),
        "the guarded comparison survived"
    );
    let sdiv = |m: &Module| {
        count(func(m, "g"), |i| {
            matches!(i, Inst::Bin { op: BinOp::SDiv, .. } | Inst::Bin { op: BinOp::SRem, .. })
        })
    };
    assert!(
        sdiv(&vrp_module(cases[1].0, true)) < sdiv(&vrp_module(cases[1].0, false)),
        "the proven-non-negative division survived"
    );
}
