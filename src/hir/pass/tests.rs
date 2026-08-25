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
use super::super::{BinOp, Func, Inst, Module, Operand, Term, build, verify};
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
        super::run_module(&mut m);
        let once: Vec<usize> = m.funcs.iter().map(ninsts).collect();
        super::run_module(&mut m);
        let twice: Vec<usize> = m.funcs.iter().map(ninsts).collect();
        assert_eq!(once, twice, "the ladder had not reached its fixpoint\n{}", src);
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
const SUM: &str = "int g(int *p,int n){int i,s=0;for(i=0;i<n;i++)s+=p[i];return s;}";

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
