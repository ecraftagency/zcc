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
use super::super::{BinOp, Func, Inst, Module, Term, build, verify};
use crate::testutil::frontend;

fn module(src: &str, opt: bool) -> Module {
    let ast = frontend(src);
    let mut m = build::build(&ast);
    if opt {
        super::run_module(&mut m);
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
    super::run_module(&mut b);
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
    for b in &f.blocks {
        assert!(!matches!(b.term, Term::Br(..)) || !b.insts.is_empty());
    }
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

// ── dce ────────────────────────────────────────────────────────────────────

#[test]
fn dce_removes_an_unused_computation_but_not_a_call() {
    let src = "int g(void){return 1;}int main(void){int dead=7*13;int keep=g();return keep+41;}";
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
