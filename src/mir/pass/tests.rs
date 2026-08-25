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

/// `pass/legalize.rs` — a frame offset past the addressing modes' reach is an
/// operand to legalize, not an assembler error. Its square is the identity on
/// ⟦·⟧: `Slot{s, off}` and `BaseImm{IP1, 0}` after `IP1 = &slot + off` denote
/// the same address, and IP1 is reserved, so no live value is destroyed. The
/// frames below are chosen to straddle the imm12 and scaled-offset limits.
#[test]
fn legalization_of_out_of_range_frame_offsets() {
    for n in [64usize, 600, 1200, 5000, 20000] {
        same(&format!(
            "int main(void){{char pad[{n}];int i,s=0;pad[0]=1;pad[{last}]=2;\n\
             for(i=0;i<8;i++)s+=pad[i*3]+i;return s+pad[0]+pad[{last}];}}",
            n = n,
            last = n - 1
        ));
    }
    // a spill slot beyond the reach as well: many live values over a big frame
    same(
        "int f(int x){return x+1;}\n\
         int main(void){char pad[9000];int a=f(1),b=f(2),c=f(3),d=f(4),e=f(5);\n\
         pad[0]=(char)a;pad[8999]=(char)b;return a+b+c+d+e+pad[0]+pad[8999];}",
    );
    // a DYNAMIC frame: x29 becomes the base and the outgoing area rides on sp
    same(
        "int g(int a,int b,int c,int d,int e,int f2,int g2,int h,int i){return i;}\n\
         int sum(int n){int v[n];int k,s=0;for(k=0;k<n;k++)v[k]=k;\n\
         for(k=0;k<n;k++)s+=v[k];return s+g(1,2,3,4,5,6,7,8,9);}\n\
         int main(void){return sum(12);}",
    );
}

// auto_inc (REARCH.md §8, R3.2) — the pre-allocation post-index fold. Its square
// is `⟦mir_v⟧ = ⟦autoinc(mir_v)⟧`: the fold moves a pointer bump into the load,
// changing no value. The test also asserts the pass FIRES on the canonical
// pointer-walk shape (Law 4 — a pass that never fires is worse than absent), and
// that the interpreter applies the writeback (so the equality is not vacuous).
#[test]
fn auto_inc_fires_and_preserves_meaning() {
    let src = "int main(void){int a[6]={1,2,3,4,5,6};int*p=a;int s=0;int i;\
               for(i=0;i<6;i++){s+=*p;p++;}return s;}";
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    // the HIR ladder first — mem2reg is what promotes the pointer local to a
    // value, without which isel emits slot loads and there is nothing to fold.
    crate::hir::pass::run_module(&mut h);
    // virtual MIR through the earlier pre-allocation passes, to match the real
    // pipeline order (ext, cmpelim) before auto_inc is measured.
    let mk = || {
        let mut m = crate::isel::lower(&h);
        for f in m.funcs.iter_mut() {
            crate::mir::pass::ext::run(f);
            crate::mir::pass::cmpelim::run(f);
        }
        m
    };
    let m_before = mk();
    let before = mi::new_machine(&m_before, &ast)
        .call("main", &[], &[])
        .expect("⟦mir_v⟧ trapped");
    assert_eq!(before as i32, 21, "the oracle: 1+2+3+4+5+6");

    let mut m_after = mk();
    let mut fired = false;
    for f in m_after.funcs.iter_mut() {
        crate::mir::pass::autoinc::run(f);
        for b in &f.blocks {
            for i in &b.insts {
                if let crate::mir::MInst::Load {
                    mem: crate::mir::AddrMode::PostIdx { .. },
                    ..
                } = i
                {
                    fired = true;
                }
            }
        }
    }
    assert!(fired, "auto_inc did not fire on a pointer-walk loop");
    let after = mi::new_machine(&m_after, &ast)
        .call("main", &[], &[])
        .expect("⟦autoinc(mir_v)⟧ trapped");
    assert_eq!(
        before as i32, after as i32,
        "auto_inc changed the meaning of the function"
    );
}

// shrink_wrap (REARCH.md §8, R3.3) — the prologue/epilogue move off the fast
// path. Proven on the firing configuration (the HIR ladder must run first, or g
// stays a call and the shape is different): `⟦mir_p⟧ = ⟦mir_final⟧` with the
// pass active, the value is the oracle's, and the saves DID move — the entry
// carries no callee-saved Spill while a later block does. A pass that fires
// nowhere is a Law-4 failure, so firing is asserted, not hoped for.
#[test]
fn shrink_wrap_moves_saves_off_the_fast_path() {
    let src = "int g(int x){if(x<=0)return 0;return x+g(x-1);}\
               int f(int n){if(n<0)return -1;int a=g(7);int b=g(9);return a+b;}\
               int main(void){return f(4)+f(-1);}";
    let ast = frontend(src);
    let build = || {
        let mut h = hir::build::build(&ast);
        crate::hir::pass::run_module(&mut h);
        allocated(&h).unwrap_or_else(|e| panic!("{}\n{}", e, src))
    };
    let mp = build();
    let before = mi::new_machine(&mp, &ast).call("main", &[], &[]).expect("⟦mir_p⟧");
    let mut mf = build();
    finish(&mut mf);
    for f in &mf.funcs {
        crate::mir::verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let after = mi::new_machine(&mf, &ast).call("main", &[], &[]).expect("⟦mir_final⟧");
    assert_eq!(before as i32, 72, "the oracle: g(7)+g(9)=28+45, then f(-1)=-1");
    assert_eq!(before as i32, after as i32, "shrink_wrap changed the meaning");

    // Fire check on the state right after frame+shrink_wrap — BEFORE `ldstp`
    // fuses the two moved `Spill`s into one `Pair`, which the Spill matcher below
    // would then miss.
    let mut chk = build();
    for f in chk.funcs.iter_mut() {
        crate::mir::pass::frame::run(f);
        crate::mir::pass::shrink_wrap::run(f);
    }
    let f = chk.funcs.iter().find(|f| f.name == "f").expect("f");
    let is_csr_spill = |i: &crate::mir::MInst| match i {
        crate::mir::MInst::Spill { slot, .. } => f.cs_saves.iter().any(|(s, _, _)| s == slot),
        _ => false,
    };
    assert!(!f.cs_saves.is_empty(), "f should save callee-saved registers");
    let entry_has = f.blocks[f.entry as usize].insts.iter().any(is_csr_spill);
    let other_has = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(bi, _)| *bi != f.entry as usize)
        .any(|(_, b)| b.insts.iter().any(is_csr_spill));
    assert!(
        !entry_has && other_has,
        "shrink_wrap did not move the saves off the entry"
    );
}

#[test]
fn a_no_op_extension_leaves_the_alu_operand_plain() {
    // R4.7's Law-4 residual: `ext_lattice` removed the standalone `sxtw`, but
    // an extension that rides INSIDE an operand (`add x1,x1,w0,sxtw`) is a
    // different instruction from the plain form — 2 cycles against 1 — and the
    // lattice never looked at it. `s += (x*y+k)&31` put one on the
    // loop-carried recurrence of d2_nested_loops (2.11×) for an extension that
    // provably does nothing: `and w,#31` leaves bits 63:32 zero and bit 31
    // clear, which is exactly what `sxtw` would write.
    use crate::mir::{MInst, Rhs};
    let src = "long f(int n,int x,int y){long s=0;int k;\
               for(k=0;k<n;k++)s+=(x*y+k)&31;return s;}\
               int main(void){return (int)f(9,3,4);}";
    let ast = frontend(src);
    let h = hir::build::build(&ast);
    let p = allocated(&h).unwrap();
    let f = p.funcs.iter().find(|f| f.name == "f").unwrap();
    let ext = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::Alu { b: Rhs::Extended(..), .. }))
        .count();
    assert_eq!(ext, 0, "a provably no-op extension stayed in the operand");
    same(src);
    // …and one that is NOT a no-op must stay: the value's top bits are unknown.
    same("long f(int n,int*a){long s=0;int k;for(k=0;k<n;k++)s+=a[k];return s;}\
          int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i-9;return (int)f(4,a);}");
}
