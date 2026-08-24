// The frame/layout battery (REARCH.md §10 row "frame / layout", added by the
// R0.9 audit — until then these two passes rode on the register allocator's
// square, which is not a proof of either).
//
// The obligation is `⟦mir_p⟧ = ⟦mir_final⟧`, but the two sides do not mean the
// same thing by "callee-saved":
//
//   * BEFORE frame lowering, a function has no prologue. AAPCS64 §6.1.1's
//     promise that x19–x28 / v8–v15 / x30 survive a call is a CONTRACT the
//     interpreter honors on its behalf — and the allocator has already relied
//     on it, which is why it put long-lived values there.
//   * AFTER frame lowering, the promise is kept by real Spill/Reload
//     instructions, and the interpreter honors nothing.
//
// So this equality is precisely the statement "frame lowering realizes, in
// instructions, the ABI assumption the allocator made" — plus "block layout
// changes the order and inverts conditions without changing any edge".
use crate::compile::{allocated, finish};
use crate::hir;
use crate::mir::interp as mi;
use crate::testutil::frontend;

fn same(src: &str) {
    let ast = frontend(src);
    let h = hir::build::build(&ast);
    let p = allocated(&h).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    let before = mi::new_machine(&p, &ast).call("main", &[], &[]);
    let mut fin = allocated(&h).unwrap();
    finish(&mut fin);
    for f in &fin.funcs {
        crate::mir::verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let after = mi::new_machine(&fin, &ast).call("main", &[], &[]);
    match (before, after) {
        (Ok(a), Ok(b)) => assert_eq!(
            a as i32, b as i32,
            "⟦mir_p⟧ = {} but ⟦mir_final⟧ = {}\n{}",
            a as i32, b as i32, src
        ),
        (Err(_), _) => {}
        (Ok(a), Err(e)) => panic!("⟦mir_final⟧ trapped ({:?}), ⟦mir_p⟧ = {}\n{}", e, a as i32, src),
    }
}

fn same_all(cases: &[&str]) {
    for c in cases {
        same(c);
    }
}

#[test]
fn callee_saved_preservation_is_realized_by_the_prologue() {
    // Each of these keeps a value live across a call, so the allocator puts it
    // in a callee-saved register and the prologue must be what saves it.
    same_all(&[
        "int f(int x){return x*2;} int main(void){int a=3;return f(a)+a;}",
        "int f(int x){return x+1;} int main(void){int a=1,b=2,c=3;return f(a)+f(b)+f(c)+a+b+c;}",
        "int fib(int n){return n<2?n:fib(n-1)+fib(n-2);} int main(void){return fib(12);}",
        "int g(int x){return x-1;} int h(int x){return g(x)*g(x);} int main(void){return h(5);}",
    ]);
}

#[test]
fn a_frameless_leaf_gets_no_frame_at_all() {
    // The audit's finding: a sentinel that conflated "no frame" with "not laid
    // out" charged every leaf a sub/add pair it did not need.
    let ast = frontend("int one(void){return 1;} int main(void){return one();}");
    let h = hir::build::build(&ast);
    let mut m = allocated(&h).unwrap();
    finish(&mut m);
    let one = m.funcs.iter().find(|f| f.name == "one").unwrap();
    assert!(one.laid_out);
    assert_eq!(one.frame_size, 0, "a leaf with no locals took a frame");
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    assert!(main.frame_size > 0, "main saves x30 and so needs a frame");
}

#[test]
fn layout_preserves_every_edge() {
    same_all(&[
        "int main(void){int s=0,i;for(i=0;i<10;i++)s+=i;return s;}",
        "int main(void){int s=0,i,j;for(i=0;i<5;i++)for(j=0;j<5;j++)s+=i*j;return s;}",
        "int main(void){int a=3;if(a>2)return 10;else return 20;}",
        "int main(void){int x=2,r=0;switch(x){case 1:r=10;break;case 2:r=20;case 3:r+=3;break;default:r=99;}return r;}",
        "int main(void){int i=0;again:i++;if(i<7)goto again;return i;}",
    ]);
}

#[test]
fn slots_do_not_overlap_and_respect_alignment() {
    // Frame lowering assigns every stack object a byte offset; two objects
    // sharing one byte is a silent aliasing bug no interpreter run would
    // reliably catch, so it is checked structurally.
    let ast = frontend(
        "struct P{char c;double d;int a[7];};\
         int main(void){struct P p;int i;p.c=1;p.d=2.5;for(i=0;i<7;i++)p.a[i]=i;return p.a[6];}",
    );
    let h = hir::build::build(&ast);
    let mut m = allocated(&h).unwrap();
    finish(&mut m);
    for f in &m.funcs {
        let mut spans: Vec<(i32, i32)> = f
            .slots
            .iter()
            .map(|s| (s.off, s.off + s.size.max(1) as i32))
            .collect();
        for s in &f.slots {
            assert_eq!(
                s.off % s.align.max(1) as i32,
                0,
                "{}: slot at {} violates alignment {}",
                f.name,
                s.off,
                s.align
            );
            assert!(
                s.off + s.size as i32 <= f.frame_size as i32,
                "{}: slot runs past the frame",
                f.name
            );
        }
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "{}: slots {:?} and {:?} overlap", f.name, w[0], w[1]);
        }
    }
}
