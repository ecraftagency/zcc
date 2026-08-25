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
    // THE EFFECT half. `same_all` below proves the meaning survives; on its own
    // it would stay green for a frame pass that emitted NOTHING, since a
    // function whose values were never spilled means the same either way. So
    // first: the prologue of a function that keeps a value across a call must
    // actually contain the save.
    // The callee carries a loop so the size heuristic does not inline it — with
    // it inlined, `main` has no call, nothing is live across one, and the
    // assertion below would be testing a function with no frame at all.
    let src = "int f(int x){int i,s=0;for(i=0;i<x;i++)s+=i;return s;}\
               int main(void){int a=3;return f(a)+a;}";
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let mut m = allocated(&h).unwrap();
    for f in m.funcs.iter_mut() {
        crate::mir::pass::frame::run(f);
    }
    let mn = m.funcs.iter().find(|f| f.name == "main").unwrap();
    assert!(!mn.cs_saves.is_empty(), "a value lives across a call but nothing is saved");
    let saves = mn.blocks[mn.entry as usize]
        .insts
        .iter()
        .filter(|i| matches!(i, crate::mir::MInst::Spill { .. }))
        .count();
    assert_eq!(saves, mn.cs_saves.len(), "the prologue does not save what cs_saves records");

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
    // THE EFFECT half: layout must actually LAY OUT — produce an order and turn
    // branches into fall-throughs — or `same_all` proves only that doing
    // nothing changes nothing.
    //
    // NOTE ON THIS TEST'S NAME, which the effect half exposed as an overstatement:
    // layout does NOT preserve every edge. It THREADS an empty block, so a
    // predecessor's successor changes from the empty block to that block's own
    // target (measured: bb2's successors go [11,12] → [2,12] when bb11 is an
    // empty forwarder). What it preserves is the RUN, which is what `same_all`
    // checks. The edge set is not the invariant; reachable behaviour is.
    let src = "int main(void){int s=0,i;for(i=0;i<10;i++)s+=i;return s;}";
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let mut m = allocated(&h).unwrap();
    finish(&mut m);
    let f = m.funcs.iter().find(|f| f.name == "main").unwrap();
    assert!(f.laid_out, "layout did not run");
    assert!(!f.order.is_empty(), "layout produced no block order");
    // at least one conditional branch falls through to the next block in the
    // order — the whole point of choosing an order at all
    let mut fallthrough = 0;
    for (k, b) in f.order.iter().enumerate() {
        let next = f.order.get(k + 1).copied();
        if let crate::mir::MTerm::Bcc(_, _, _, e) = &f.blocks[*b as usize].term {
            if Some(e.block) == next {
                fallthrough += 1;
            }
        }
    }
    assert!(fallthrough > 0, "no conditional branch was laid out to fall through");

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
    // THE EFFECT half: after the pass EVERY frame offset is encodable, and on a
    // frame this size at least one was not before. Without this, `same` alone
    // stays green for a legalizer that never fires.
    {
        let src = "int main(void){char pad[20000];int i,s=0;pad[0]=1;pad[19999]=2;\n\
                   for(i=0;i<8;i++)s+=pad[i*3]+i;return s+pad[0]+pad[19999];}";
        let ast = frontend(src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let mut m = allocated(&h).unwrap();
        for f in m.funcs.iter_mut() {
            crate::mir::pass::frame::run(f);
        }
        let off_ok = |f: &crate::mir::MFunc| {
            f.blocks.iter().flat_map(|b| b.insts.iter()).all(|i| match i {
                crate::mir::MInst::Load { op, mem: crate::mir::AddrMode::Slot { slot, off }, .. }
                | crate::mir::MInst::Store { op, mem: crate::mir::AddrMode::Slot { slot, off }, .. } => {
                    crate::mir::isa::mem_off_ok(f.slots[*slot as usize].off + off, op.bytes())
                }
                _ => true,
            })
        };
        let before = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(!off_ok(before), "this frame was supposed to have an unencodable offset");
        // MModule is not Clone, so the "after" side is built the same way and
        // then legalized — the two differ by exactly this pass.
        let mut m2 = allocated(&h).unwrap();
        for f in m2.funcs.iter_mut() {
            crate::mir::pass::frame::run(f);
            crate::mir::pass::legalize::run(f);
        }
        let after = m2.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(off_ok(after), "legalize left an unencodable frame offset");
    }
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

#[test]
fn one_epilogue_serves_every_return_of_a_shape() {
    // R4.4. Every `Ret` block used to carry its own copy of the callee-saved
    // reloads, and `emit` adds `add sp` and `ret` to each: sqlite paid 3,815
    // `ret` against gcc's 317. The tails are identical — physical registers,
    // fixed slots — and the return value is already in its ABI register, so a
    // shared epilogue observes nothing about which path reached it.
    use crate::mir::MTerm;
    // The call comes FIRST, so every return path is inside the region
    // shrink-wrapping would pick and all four tails are the same shape. (When
    // shrink-wrapping does fire, the fast path's returns have no reloads at all
    // — a different shape, deliberately kept apart.)
    let src = "int g(int);\
               int f(int n){int s=g(n)+g(n+1);\
                            if(s<0)return -1;if(s==0)return 0;\
                            if(s>100)return s;return s*2;}\
               int g(int x){int i,s=0;for(i=0;i<x;i++)s+=i*3;return s;}\
               int main(void){return f(2)+f(-1)+f(0);}";
    same(src);
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let mut m = allocated(&h).unwrap();
    // Checked right after frame + shrink_wrap + the merge, BEFORE `ldstp` fuses
    // the reloads into a `Pair` the matcher below would miss — the same reason
    // `shrink_wrap_moves_saves_off_the_fast_path` stops there.
    for f in m.funcs.iter_mut() {
        crate::mir::pass::frame::run(f);
        crate::mir::pass::shrink_wrap::run(f);
        crate::mir::pass::frame::merge_epilogues(f);
    }
    let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
    let rets = f.blocks.iter().filter(|b| matches!(b.term, MTerm::Ret)).count();
    let paths = f
        .blocks
        .iter()
        .filter(|b| matches!(b.term, MTerm::Ret | MTerm::B(_)))
        .count();
    assert!(rets < 4 && rets < paths, "{} return paths still end in {} `ret`s", paths, rets);
    // The callee-saved tail exists ONCE, not once per path. (More than one
    // `ret` may survive: shrink-wrapping leaves the fast path's returns with no
    // reloads at all, and a bare `ret` is shorter than the branch that would
    // replace it — different shapes, deliberately not merged together.)
    let cs: std::collections::HashSet<u32> = f.cs_saves.iter().map(|(s, _, _)| *s).collect();
    let reloads = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, crate::mir::MInst::Reload { slot, .. } if cs.contains(slot)))
        .count();
    assert_eq!(reloads, cs.len(), "the callee-saved tail is duplicated");
    // …and a frame slot nothing names occupies nothing, so a leaf whose locals
    // were all promoted carries no frame at all.
    let src = "int leaf(int a,int b){int x=a+b;int y=a-b;return x*y;}\
               int main(void){return leaf(5,3);}";
    same(src);
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let mut m = allocated(&h).unwrap();
    finish(&mut m);
    let l = m.funcs.iter().find(|f| f.name == "leaf").unwrap();
    assert_eq!(l.frame_size, 0, "every local was promoted, so there is no frame");
}

#[test]
fn a_constant_already_materialized_is_not_materialized_again() {
    // R4.6. HIR carries a constant as an operand, not a value — which is what
    // lets isel fold it into an immediate field without proving single use — so
    // at the one place it does NOT fold, isel mints a fresh `MovImm` per use and
    // nothing shares them. sqlite paid 9,035 repeated immediates for it.
    use crate::mir::MInst;
    let src = "int f(int*p,int n){int i,s=0;for(i=0;i<n;i++){s+=p[i]*1000003;p[i]=1000003;}return s;}\
               int main(void){int a[3];int i;for(i=0;i<3;i++)a[i]=i;return f(a,3)&255;}";
    same(src);
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let m = allocated(&h).unwrap();
    let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
    let movs = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::MovImm { imm: 1000003, .. }))
        .count();
    assert_eq!(movs, 1, "the same literal was materialized {} times", movs);

    // …but NOT across a call: AAPCS64 §6.1.1 leaves ten callee-saved GPRs, and a
    // constant is the one value for which competing for one is never worth it —
    // re-materializing costs one instruction. Shipped without this, thirteen
    // csmith programs failed to allocate ("11 call-crossing Gpr values live but
    // only 10 callee-saved").
    let src = "int g(int);\
               int f(int x){int a=g(x+1000003);int b=g(a+1000003);return a+b;}\
               int g(int x){int i,s=0;for(i=0;i<3;i++)s+=x+i;return s;}\
               int main(void){return f(1)&255;}";
    same(src);
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let m = allocated(&h).unwrap();
    let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
    let movs = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::MovImm { imm: 1000003, .. }))
        .count();
    assert_eq!(movs, 2, "a constant was shared across a call");
}

#[test]
fn a_pair_replaces_two_adjacent_accesses() {
    // `ldp`/`stp` (DDI 0487 C6.2.130). The square this replaces asserted only
    // `equiv(...)` on three programs — which a pairing pass that never fired
    // would also pass, and that is exactly the vacuous square §17 was full of.
    // So: the pair must APPEAR where the shape allows it, and must NOT appear
    // where the ISA has no paired form.
    use crate::mir::MInst;
    let pairs = |src: &str, f: &str| -> usize {
        let ast = frontend(src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let mut m = allocated(&h).unwrap();
        finish(&mut m);
        m.funcs
            .iter()
            .find(|x| x.name == f)
            .map(|x| {
                x.blocks
                    .iter()
                    .flat_map(|b| b.insts.iter())
                    .filter(|i| matches!(i, MInst::Pair { .. }))
                    .count()
            })
            .unwrap_or(0)
    };
    // Two values already in DISTINCT registers, stored to adjacent slots. The
    // literal version of this (`p[0]=1;p[1]=2`) does NOT pair, and correctly so:
    // both stores use the same transfer register, reloaded between them, and
    // moving the second back would read the wrong value. gcc schedules the two
    // materializations first; zcc does not. Recorded as a residual, not asserted
    // here as a failure of this pass.
    let two_longs = "void g(long*p,long x,long y){p[0]=x;p[1]=y;}\
                     int main(void){long a[2];g(a,1,2);return (int)(a[0]*10+a[1]);}";
    assert!(pairs(two_longs, "g") >= 1, "two adjacent 8-byte stores did not pair");
    same(two_longs);

    // A BYTE access has no paired form: `stp` exists at 32 and 64 bits and for
    // S/D/Q, and nowhere else. Nothing may be invented here.
    let two_bytes = "void g(char*p){p[0]=1;p[1]=2;}\
                     int main(void){char a[2];g(a);return a[0]*10+a[1];}";
    assert_eq!(pairs(two_bytes, "g"), 0, "a byte access has no paired form");
    same(two_bytes);

    // RESIDUAL, measured here and recorded rather than fixed: `p->a + p->b`
    // emits two adjacent loads off one base and does NOT pair, because `fuse`
    // refuses when a destination equals the base register. DDI 0487 C6.2.130
    // constrains that only for the WRITEBACK forms — plain `ldp x1, x0, [x0]`
    // reads the base once to form the address and is well defined, so this is a
    // missed pair, not an illegal one. The square still holds; only the count
    // is short.
    same("struct P{long a,b;};long f(struct P*p){return p->a+p->b;}\
          int main(void){struct P p;p.a=40;p.b=2;return (int)f(&p);}");
}

#[test]
fn an_arithmetic_result_needs_no_second_compare() {
    // cmp_elim's square, which did not exist — the pass shipped with no proof
    // at all, found by `tests/provenance.sh`.
    //
    // A64's `subs`/`adds`/`ands` set NZCV from their own result, so `sub w0,..`
    // followed by `cmp w0, #0` computes the flags twice. THE CONDITION CODE IS
    // THE WHOLE PROBLEM: `cmp d,#0` leaves C=1 and V=0 by definition, while
    // `subs` sets both from the arithmetic, so only the codes reading N and Z
    // survive — `lt` becoming `mi` and `ge` becoming `pl`, which is the rewrite
    // this test's second half exercises.
    use crate::mir::MInst;
    let build_one = |src: &str, f: &str| -> crate::mir::MFunc {
        let ast = frontend(src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let mut m = crate::isel::lower(&h);
        for g in m.funcs.iter_mut() {
            crate::mir::pass::cmpelim::run(g);
        }
        m.funcs.into_iter().find(|x| x.name == f).unwrap()
    };
    // The flags must be CONSUMED by something cmp_elim can reach — a select.
    // `if (d == 0)` does not exercise this pass at all: isel already turns that
    // into `cbz` with no `cmp` instruction to eliminate, which is why the first
    // attempt at this test asserted against an empty set.
    let src = "int f(int a,int b){int d=a-b;return d<0?11:22;}\
               int main(void){return f(1,2)*100+f(2,1);}";
    let g = build_one(src, "f");
    let flagged = g
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::Alu { flags: Some(_), .. }))
        .count();
    let cmps = g
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::Cmp { .. }))
        .count();
    assert_eq!(flagged, 1, "the subtraction did not take over the compare");
    assert_eq!(cmps, 0, "a redundant `cmp #0` survived");
    // …and the condition was REWRITTEN, not inherited: `subs` sets V from the
    // arithmetic where `cmp d,#0` left it 0, so `lt` is only `mi`.
    let mi = g
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::CSel { cc: crate::mir::CC::Mi, .. }))
        .count();
    assert_eq!(mi, 1, "`lt` was not rewritten to `mi` when V stopped being 0");
    same(src);

    // `ands` reaches it too, and there the condition reads N and Z alone, so it
    // survives unchanged.
    let src2 = "int f(int a,int b,int c){int d=a&b;return d==0?c:d;}\
                int main(void){return f(3,4,9)*100+f(3,1,9);}";
    let g2 = build_one(src2, "f");
    assert_eq!(
        g2.blocks.iter().flat_map(|b| b.insts.iter())
            .filter(|i| matches!(i, MInst::Cmp { .. })).count(),
        0,
        "a redundant `cmp #0` survived after `and`"
    );
    same(src2);
}
