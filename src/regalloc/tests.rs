// The register-allocator battery (REARCH.md §7.6, §10 row "regalloc").
//
// The obligation is a RENAMING BISIMULATION: allocation renames values and may
// route some of them through memory, but it must not change what the function
// computes. So the battery runs ⟦mir_v⟧ (before allocation) and ⟦mir_p⟧ (after)
// and requires equality — the same shape of proof as isel's, one layer down.
//
// Two structural properties are checked alongside, because they are decidable
// here and would otherwise only show up as a wrong answer: `regalloc::verify`
// (no vreg survives, every reload dominated by its spill, no parallel copy
// left) and the coloring's own claim that it never needs more colours than the
// measured maximum pressure.
use crate::hir;
use crate::isel;
use crate::mir::interp as mi;
use crate::testutil::frontend;

/// Compile to virtual MIR, allocate, and require both to compute the same value.
/// Run on both sides of the §4 pass ladder: optimization changes the shape of
/// the live ranges the allocator sees (longer values, more pressure), so the
/// unoptimized side alone would not exercise it the way real code does.
fn same(src: &str) {
    same_side(src, false);
    same_side(src, true);
}

fn same_side(src: &str, opt: bool) {
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    if opt {
        hir::pass::run_module(&mut h);
    }
    let h = h;
    for f in &h.funcs {
        hir::verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let v = isel::lower(&h);
    let before = {
        let mut mach = mi::new_machine(&v, &ast);
        mach.call("main", &[], &[])
    };
    // The allocator's obligations are checked INSIDE `allocated`, which is where
    // they are expressible: obligation (b) — every `Reload` reads a slot some
    // `Spill` wrote on every path — is stated in the Spill/Reload vocabulary, and
    // frame lowering spends that vocabulary. `ldstp` folds two spills into one
    // `stp`, which carries an address rather than a slot number, so a store-set
    // gathered after `finish` is missing them and reports `reload of unstored
    // slot` on a function whose answer is right (measured: the nine rotation
    // shapes below all agree with gcc -O1 at -O0 and -O1). Law 3 says certify at
    // the EARLIEST layer where the question is decidable; for this obligation
    // that layer is post-allocation, pre-frame-lowering.
    let p = crate::compile::backend(&h).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    let after = {
        let mut mach = mi::new_machine(&p, &ast);
        mach.call("main", &[], &[])
    };
    match (before, after) {
        (Ok(a), Ok(b)) => assert_eq!(
            a as i32, b as i32,
            "⟦mir_v⟧ = {} but ⟦mir_p⟧ = {}\n{}",
            a as i32, b as i32, src
        ),
        (Err(_), _) => {}
        (Ok(a), Err(e)) => panic!("⟦mir_p⟧ trapped ({:?}), ⟦mir_v⟧ = {}\n{}", e, a as i32, src),
    }
}

fn same_all(cases: &[&str]) {
    for c in cases {
        same(c);
    }
}

#[test]
fn straight_line_and_loops() {
    same_all(&[
        "int main(void){return 42;}",
        "int main(void){int a=7,b=5;return a*b-a/b;}",
        "int main(void){int s=0,i;for(i=0;i<10;i++)s+=i;return s;}",
        "int main(void){int i=0,s=0;while(i<5){s+=i*i;i++;}return s;}",
        "int main(void){int i=0,s=0;do{s+=i;i++;}while(i<5);return s;}",
        "int main(void){int s=0,i,j;for(i=0;i<5;i++)for(j=0;j<5;j++)s+=i*j;return s;}",
    ]);
}

#[test]
fn block_arguments_become_edge_copies() {
    // Every `?:` and short-circuit is a block parameter, and every one of them
    // must survive destruction as a parallel copy on the edge.
    same_all(&[
        "int main(void){int a=1,b=2;return (a<b?a:b)+(a>b?a:b);}",
        "int main(void){int a=0,b=3;return (a&&b)+(a||b)+(a&&!b);}",
        "int main(void){int i=3;return i>0 ? (i>2 ? 7 : 8) : 9;}",
        "int main(void){int s=0,i;for(i=0;i<8;i++)s+=(i&1)?i:-i;return s;}",
        "double m(double a,double b){return a<b?a:b;} int main(void){return (int)m(2.0,5.0);}",
    ]);
}

#[test]
fn calls_place_arguments_and_preserve_live_values() {
    // A value live across a call may not sit in a caller-saved register — the
    // single rule that replaces rc3's "crossing" special case.
    same_all(&[
        "int f(int x){return x*2;} int main(void){int a=3;return f(a)+a;}",
        "int f(int x){return x+1;} int main(void){int a=1,b=2,c=3;return f(a)+f(b)+f(c)+a+b+c;}",
        "int fib(int n){return n<2?n:fib(n-1)+fib(n-2);} int main(void){return fib(12);}",
        "int a8(int a,int b,int c,int d,int e,int f,int g,int h){return a+b+c+d+e+f+g+h;}\
         int main(void){return a8(1,2,3,4,5,6,7,8);}",
        // the argument permutation that needs a real parallel copy
        "int sub(int a,int b){return a-b;} int main(void){int x=10,y=3;return sub(y,x)+sub(x,y);}",
        "double d2(double a,double b){return a-b;} int main(void){return (int)d2(5.5,1.5);}",
        "int g(void){return 3;} int main(void){int (*p)(void)=g;return p()*14;}",
    ]);
}

#[test]
fn memory_and_globals() {
    same_all(&[
        "int main(void){int a[5],i;for(i=0;i<5;i++)a[i]=i*i;return a[4];}",
        "int g=10; int main(void){g+=5;return g;}",
        "int a[3]={1,2,3}; int main(void){return a[0]+a[1]+a[2];}",
        "int main(void){char s[]=\"abc\";return s[0]+s[2];}",
        "struct P{int x,y;}; int main(void){struct P p;p.x=3;p.y=4;return p.x*p.y;}",
    ]);
}

#[test]
fn switch_chains() {
    same_all(&[
        "int main(void){int x=2,r=0;switch(x){case 1:r=10;break;case 2:r=20;case 3:r+=3;break;default:r=99;}return r;}",
        "int main(void){int s=0,i;for(i=0;i<6;i++){switch(i){case 0:case 2:case 4:s+=i;break;default:s-=1;}}return s;}",
    ]);
}

// promote (REARCH.md §13p, R4.16) — region-resident spill: a memory-resident
// value a wholly-free callee-saved register could hold goes back to a register.
#[test]
fn promote_moves_a_spilled_value_out_of_memory() {
    // THE MEANING half. These force spilling (many values live across calls) and
    // exercise the fixed-use path specifically: a promoted value read into an ABI
    // argument register before a call must keep its `mov` and NOT be propagated
    // into the call — the miscompile the guard fixes (torture 20180921-1).
    same_all(&[
        "int e(int x){return x+1;}\
         int hot(int p){int a=e(p),b=e(p),c=e(p),d=e(p),f=e(p),g=e(p);\
           return e(p)+a+b+c+d+f+g+e(p)*p+p;}\
         int main(void){return hot(3);}",
        "int e(int a,int b){return a-b;}\
         int hot(int p){int s=0,i;for(i=0;i<8;i++)s+=e(p,i)+e(i,p);return s+p;}\
         int main(void){return hot(5);}",
        "int fib(int n){return n<2?n:fib(n-1)+fib(n-2);} int main(void){return fib(13);}",
    ]);

    // THE EFFECT half — otherwise the meaning half stays green for a pass that
    // promotes NOTHING. Promotion fires only where the allocator LEFT a
    // callee-saved register free while spilling, a suboptimality that appears at
    // scale (sqlite3VdbeExec) but not in a small clean function, so the mechanism
    // is exercised on a hand-built physical function: one spill store dominating
    // three reloads, x19–x28 all free. Promotion must convert every reload out of
    // memory (no `Reload` survives), route the value through a callee-saved
    // register (a `Copy` reads one), and record it in `saved` so `frame` preserves
    // it.
    use crate::mir::*;
    let alu = |dst: u8, a: u8| MInst::Alu {
        op: AluOp::Add,
        w: Width::W64,
        dst: Reg::P(PReg::gpr(dst)),
        a: Reg::P(PReg::gpr(a)),
        b: Rhs::Imm(1),
        flags: None,
    };
    let reload = |dst: u8| MInst::Reload { slot: 0, dst: Reg::P(PReg::gpr(dst)), w: Width::W64 };
    let blk = |insts: Vec<MInst>, term: MTerm| MBlock {
        params: Vec::new(),
        insts,
        term,
        weight: 1,
        labels: Vec::new(),
    };
    let to = |b: MBlockId| MTarget { block: b, args: Vec::new() };
    let mut f = MFunc {
        name: "hot".into(),
        blocks: vec![
            blk(
                vec![
                    MInst::MovImm { w: Width::W64, dst: Reg::P(PReg::gpr(0)), imm: 5 },
                    MInst::Spill { slot: 0, src: Reg::P(PReg::gpr(0)), w: Width::W64 },
                ],
                MTerm::B(to(1)),
            ),
            blk(vec![reload(1), alu(2, 1)], MTerm::B(to(2))),
            blk(vec![reload(3), alu(4, 3), reload(5), alu(6, 5)], MTerm::Ret),
        ],
        vregs: Vec::new(),
        slots: vec![StackSlot { size: 8, align: 8, kind: SlotKind::Spill, off: 0 }],
        entry: 0,
        is_static: false,
        is_weak: false,
        order: Vec::new(),
        laid_out: false,
        frame_size: 0,
        saved: RegSet::default(),
        dyn_stack: false,
        has_vla: false,
        outgoing: 0,
        fp_slot: 0,
        cs_saves: Vec::new(),
        physical: true,
    };
    super::promote::run(&mut f);

    let reloads_left = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::Reload { .. }))
        .count();
    assert_eq!(reloads_left, 0, "promotion left a reload in memory");
    let via_callee = f.blocks.iter().flat_map(|b| b.insts.iter()).any(|i| {
        matches!(i, MInst::Copy { src: Reg::P(p), .. } if isa::is_callee_saved(*p))
    });
    assert!(via_callee, "the value was not routed through a callee-saved register");
    let saved_a_callee = f.saved.iter().any(isa::is_callee_saved);
    assert!(saved_a_callee, "the promoted register was not added to `saved`");
}

#[test]
fn high_pressure_forces_the_spiller() {
    // More simultaneously live values than k = 26 general registers, so the
    // spiller must fire and the coloring must still succeed.
    let mut src = String::from("int main(void){\n");
    for i in 0..40 {
        src.push_str(&format!("int v{} = {};\n", i, i * 3 + 1));
    }
    // one expression reading them all, so all 40 are live at once
    src.push_str("int s = 0;\n");
    src.push_str("s = ");
    for i in 0..40 {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("v{}*{}", i, i + 1));
    }
    src.push_str(";\nreturn s & 0xff;\n}\n");
    same(&src);
}

#[test]
fn parallel_copy_cycles_are_broken_with_the_scratch_register() {
    // A swap across a loop back edge is the classic case where sequentializing
    // naively would clobber: the windmill must route through x16.
    same_all(&[
        "int main(void){int a=1,b=2,i;for(i=0;i<5;i++){int t=a;a=b;b=t;}return a*10+b;}",
        "int main(void){int a=1,b=2,c=3,i;for(i=0;i<7;i++){int t=a;a=b;b=c;c=t;}return a*100+b*10+c;}",
    ]);
}

/// The spiller's TWO ceilings (REARCH §7.2/§7.3), both of which the colourer
/// depends on and neither of which is visible in a small function:
///
///   1. total pressure ≤ k at every point, where a call's clobber set counts as
///      fixed definitions live across it;
///   2. the number of live CALL-CROSSING values ≤ the callee-saved count of the
///      class — checked at every point, not only at the calls. Two values may
///      cross DIFFERENT calls and still be live together in between, and the
///      colourer would then need more callee-saved colours than exist.
///
/// The second ceiling was missing, and a long-double prologue (one conversion
/// call per parameter, every converted value live to the end) is exactly the
/// shape that exposes it.
#[test]
fn spilling_respects_both_ceilings() {
    // 12 integer values, each defined by a call and all live to the end: more
    // call-crossing values than x19–x28 can hold
    let mut src = String::from("int f(int x){return x*2+1;}\nint main(void){\n");
    for i in 0..12 {
        src.push_str(&format!("int v{i}=f({i});\n"));
    }
    src.push_str("return ");
    for i in 0..12 {
        src.push_str(&format!("v{i}{}", if i == 11 { ";}\n" } else { "+" }));
    }
    same(&src);

    // the same shape in the FP file, where only v8–v15 survive a call
    let mut src = String::from("double f(double x){return x*2.0+1.0;}\nint main(void){\n");
    for i in 0..12 {
        src.push_str(&format!("double v{i}=f({i}.0);\n"));
    }
    src.push_str("return (int)(");
    for i in 0..12 {
        src.push_str(&format!("v{i}{}", if i == 11 { ");}\n" } else { "+" }));
    }
    same(&src);

    // long double: one conversion call per parameter, ten values live at once
    same(
        "long double many(long double a,long double b,long double c,long double d,\
         long double e,long double f,long double g,long double h,long double i,long double j)\
         {return a+b+c+d+e+f+g+h+i+j;}\n\
         int main(void){return (int)many(1.L,2,3,4,5,6,7,8,9,10);}",
    );

    // mixed files at once, so neither ceiling can be satisfied by luck
    same(
        "int fi(int x){return x+1;}double fd(double x){return x+1.0;}\n\
         int main(void){int a=fi(1),b=fi(2),c=fi(3),d=fi(4),e=fi(5),g=fi(6),h=fi(7),\
         i=fi(8),j=fi(9),k=fi(10),l=fi(11),m=fi(12);\n\
         double p=fd(1),q=fd(2),r=fd(3),s=fd(4),t=fd(5),u=fd(6),v=fd(7),w=fd(8),\
         x=fd(9),y=fd(10);\n\
         return a+b+c+d+e+g+h+i+j+k+l+m+(int)(p+q+r+s+t+u+v+w+x+y);}",
    );
}


#[test]
fn a_join_wider_than_the_register_file_is_relieved_by_the_slot() {
    // mem2reg gives a join one parameter per live local, so an edge can carry
    // more arguments than the class has registers — and an argument is a use AT
    // the terminator, which no eviction can relieve. The spiller answers by
    // removing the PARAMETER: the value travels through its slot instead.
    let mut src = String::from("int main(void){\nint s=0;\n");
    for i in 0..40 {
        src.push_str(&format!("int v{} = {};\n", i, i + 1));
    }
    src.push_str("if (s == 0) { s = 1; } else { s = 2; }\n");
    // every value is live ACROSS the join
    src.push_str("return (");
    for i in 0..40 {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("v{}", i));
    }
    src.push_str(") + s;\n}\n");
    same(&src);
}

#[test]
fn a_value_crossing_many_calls_fits_the_callee_saved_file() {
    // AAPCS64 §6.1.1 leaves ten allocatable callee-saved general registers, and
    // `color.rs` applies that restriction to a VALUE over its whole range. More
    // than ten live call-crossing values therefore cannot be coloured, however
    // much room the class otherwise has — the spiller has to see the ceiling
    // separately from the k ceiling.
    let mut src = String::from("int g(int);\nint main(void){\n");
    for i in 0..16 {
        src.push_str(&format!("int a{} = g({});\n", i, i));
    }
    src.push_str("return ");
    for i in 0..16 {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("a{}", i));
    }
    src.push_str(";\n}\nint g(int x){return x;}\n");
    same(&src);
}

#[test]
fn a_rematerializable_value_is_recomputed_rather_than_stored() {
    // A producer that reads no register (`MovImm`, `Adrp`, `SlotAddr`) is
    // cheaper to re-execute than to store and reload, and it needs no slot at
    // all. The battery's job is the square — the count is checked on the corpus.
    let mut src = String::from("int main(void){\nint s=0;\n");
    for i in 0..40 {
        src.push_str(&format!("int c{} = {};\n", i, 1000 + i * 7));
    }
    src.push_str("s = ");
    for i in 0..40 {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("c{}", i));
    }
    src.push_str(";\nreturn s & 0x7f;\n}\n");
    same(&src);
}

#[test]
fn a_loop_carried_variable_survives_being_spilled() {
    // The shape that broke first: a loop whose carried values exceed the file,
    // so the parameter is evicted and the incoming argument is stored on every
    // edge. Slot coalescing makes most of those stores no-ops; the value must
    // still be right.
    let mut src = String::from("int main(void){\nint i;\n");
    for k in 0..30 {
        src.push_str(&format!("int a{} = {};\n", k, k));
    }
    src.push_str("for(i=0;i<7;i++){\n");
    for k in 0..30 {
        src.push_str(&format!("a{} = a{} + i;\n", k, k));
    }
    src.push_str("}\nreturn (");
    for k in 0..30 {
        if k > 0 {
            src.push('+');
        }
        src.push_str(&format!("a{}", k));
    }
    src.push_str(") & 0xff;\n}\n");
    same(&src);
}

#[test]
fn a_branch_condition_live_across_a_call_is_callee_saved() {
    // The condition is used ONLY by the terminator, so it never appears in
    // `live_out` — and the backward walk that decides "crosses a call" started
    // from `live_out`. The value was therefore invisible to the rule and got a
    // caller-saved register the call destroyed. Latent while every local was a
    // memory cell: the condition was reloaded immediately before the branch.
    // (torture pr36343.)
    same(
        "int g(int);\
         int f(int c){int t=g(1);if(c)return t+1;return t+2;}\
         int main(void){return f(0)==2?0:1;}\
         int g(int x){return x;}",
    );
}

#[test]
fn an_indirect_callee_may_share_the_result_register() {
    // `blr x0` reads the target before the call writes the result into x0, so
    // the two may share — but only ONE of them dies there. The colourer's
    // occupied set is a multiset for exactly this reason; collapsing the two
    // holders into one entry made the survivor vanish with the corpse.
    // (torture pr34768-2.)
    same(
        "int a(void){return 1;}int b(void){return 2;}\
         int f(int c){int t=7;int r=(c?a:b)();return t+r;}\
         int main(void){return f(1)==8?0:1;}",
    );
}

#[test]
fn a_coalescing_hint_never_offers_a_reserved_register() {
    // Biased colouring takes a copy partner's register, and a partner can be a
    // PHYSICAL one the allocator would never offer: an integer constant zero is
    // `Reg::P(ZR)`, so an edge argument holding one would hand the zero register
    // to a real value — `cmp wzr, #5` for a loop counter that is never anything
    // but zero. The hint is filtered through `alloc_mask` for that reason, and
    // `color::check` refuses a non-allocatable colour outright.
    same("int main(void){int a=1,b=2,i;for(i=0;i<5;i++){int t=a;a=b;b=t;}return a*10+b;}");
    same("int main(void){int s=0,i;for(i=0;i<4;i++){int j=0;while(j<i){s+=j;j++;}}return s;}");
}

#[test]
fn a_w_form_extension_is_not_a_64_bit_one() {
    // `sxtb w0, w1` sign-extends inside the low 32 bits and ZEROES bits 63:32
    // (DDI 0487 B1.2.1). Recording that as "sign-extended from 8 bits" makes a
    // later `sxtw` look redundant when it is precisely the instruction that
    // would fill the upper half — a wrong-code bug on every negative value
    // (yarpgen s0009 and forty-four others). `same` runs the whole backend, so
    // the MIR extension lattice is under test here.
    same("int main(void){signed char c=-3;int i=c;long l=i;return (int)(l>>32)==-1;}");
    same("int main(void){short s=-2;int i=s;long l=i;return (int)(l>>32)==-1;}");
    same("int main(void){signed char c=-1;unsigned u=(unsigned char)c;long l=u;return (int)(l==255);}");
    same("int main(void){signed char c=-5;long l=c;return (int)(l>>40)==-1;}");
    same("int f(int x){signed char c=(signed char)x;long l=c;return (int)(l>>32);}\
          int main(void){return f(-1)+1;}");
}

#[test]
fn a_truncating_self_move_is_not_a_no_op() {
    // DDI 0487 B1.2.1: every 32-bit write ZEROES bits 63:32, so `mov w0, w0`
    // TRUNCATES — it is redundant only when the source was itself produced at 32
    // bits. Deleting it unconditionally left the upper half of a 64-bit value
    // alive under a 32-bit name. Latent until biased colouring started handing a
    // copy its source's own register on purpose (yarpgen s0131 and nine others).
    same("unsigned f(unsigned long x){unsigned a=(unsigned)x;return a;}\
          int main(void){return f(0x1234567800000042UL)==0x42u?0:1;}");
    same("int main(void){unsigned long x=0xffffffff00000001UL;unsigned a=x;unsigned long y=a;\
          return y==1?0:1;}");
    same("unsigned g(unsigned long a,unsigned long b){unsigned x=a;unsigned y=b;return x+y;}\
          int main(void){return g(0xff00000001UL,0xee00000002UL)==3u?0:1;}");
}

#[test]
fn a_chain_of_narrowing_copies_keeps_one_truncation() {
    // `mov w0, w0` TRUNCATES. Deleting a self-move is right only when nobody
    // reads the register wider — and that question is not local: with
    // `t1 = (int)x; t2 = t1; use64(t2)`, `t1` looks 32-bit-only until `t2`'s copy
    // is deleted and `t1` inherits its 64-bit reader. The decision is a fixpoint
    // for that reason (yarpgen s0131).
    same("long f(long x){int t=(int)x;unsigned u=(unsigned)t;return (long)u;}\
          int main(void){return f(0x1234567800000005L)==5?0:1;}");
    same("unsigned long f(long x){int a=(int)(-x);unsigned b=a;return (unsigned long)b;}\
          int main(void){return f(0x100000001L)==0xffffffffUL?0:1;}");
    same("long g(long a,long b){int x=(int)a;int y=x;long z=(unsigned)y;return z-b;}\
          int main(void){return g(0xff00000007L,7)==0?0:1;}");
}

/// R4.1 — a reload copy is CARRIED into the blocks its definition dominates, so
/// a memory-resident value wanted in a chain of nested blocks is reloaded ONCE
/// instead of once per block. The semantic battery above cannot see this: eight
/// reloads and one reload compute the same number. So the COUNT is what is
/// pinned here, and the count is the whole point of the step.
///
/// The condition that makes the carry sound is the same one that makes it fire.
/// A copy is kept across an edge only where EVERY predecessor is holding it
/// under the same name; a copy has exactly one definition; so every path from
/// the entry to the use runs through that definition — which is dominance, and
/// SSA therefore holds by construction with no reconstruction and no block
/// parameter. `mir::verify` re-derives that independently after the spiller runs.
///
/// The shape matters and the first draft got it wrong twice. The values must be
/// LOADED, since a constant is rematerializable and never reaches the spiller at
/// all; and the pressure that forces `x` into memory must be BEHIND the nested
/// uses rather than around them, or the copy is evicted again immediately and
/// the test measures nothing.
#[test]
fn a_reload_is_carried_into_the_blocks_it_dominates() {
    let mut src = String::from("int g[64];\nint main(void){\nint s = 0;\nint x = g[0] * 3 + 1;\n");
    // (1) a region that exhausts the register file while `x` is live across it,
    //     so `x` is certain to be memory-resident afterwards
    for i in 0..40 {
        src.push_str(&format!("int v{} = g[{}] * {} + 1;\n", i, i, i + 2));
    }
    src.push_str("s += ");
    for i in 0..40 {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("v{}*{}", i, i + 1));
    }
    src.push_str(";\n");
    // (2) NESTED uses of `x`, each block dominated by the one above it and the
    //     pressure now gone: one reload should serve all eight
    for i in 0..NEST {
        src.push_str(&format!("if (g[{}] > 0) {{ s += x;\n", i + 41));
    }
    for _ in 0..NEST {
        src.push('}');
    }
    src.push_str("\nreturn s & 0xff;\n}\n");

    same(&src);

    let ast = frontend(&src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let p = crate::compile::backend(&h).unwrap();
    let f = p.funcs.iter().find(|f| f.name == "main").unwrap();
    let mut per_slot: std::collections::BTreeMap<crate::mir::SlotId, usize> =
        std::collections::BTreeMap::new();
    for inst in f.blocks.iter().flat_map(|b| b.insts.iter()) {
        if let crate::mir::MInst::Reload { slot, .. } = inst {
            *per_slot.entry(*slot).or_default() += 1;
        }
    }
    // `x`'s slot is the one wanted in all NEST blocks. Before the carry it was
    // reloaded once per block; the assertion is on the WORST slot, so it does not
    // depend on which slot number `x` happens to receive, and it fails in both
    // directions — a regression that reinstates the per-block reload, and an
    // over-claim that no slot is reloaded at all.
    let worst = per_slot.values().copied().max().unwrap_or(0);
    assert!(
        worst > 0 && worst < NEST,
        "worst slot reloaded {} times over {} nested uses — the copy is not being carried",
        worst,
        NEST
    );
}

/// How deep the nested uses go. Large enough that "once per block" and "once"
/// cannot be confused with each other or with allocator noise.
const NEST: usize = 8;

/// R4-capstone (spec §4.4) — the SEQUEL to the carry above, one edge kind
/// further. R4.1 carries a residency across a FORWARD edge, where every
/// predecessor has already been simulated. A loop header's latch has not: blocks
/// are walked in reverse postorder, so when the header is simulated the latch
/// "holds nothing" and residency restarts every iteration. Lifting that needs a
/// second fixpoint — register-residency propagated backwards, read one round
/// behind (`spill::backedge_entry_residency`).
///
/// This test is the MEANING GUARD for that fixpoint, written before it and kept
/// after it: whatever the second lattice decides to keep in a register around a
/// loop, ⟦mir_v⟧ = ⟦mir_p⟧ must not move. The EFFECT — the reload count at the
/// header dropping — cannot be asserted yet: the carry it enables is unsound
/// until the block-parameter that reconciles the two reaching definitions exists
/// (Task 2's `reconstruct::insert_phi`), so it ships flag-OFF and the count is
/// pinned by the task that turns the flag on.
///
/// The callee `e` is DEFINED rather than declared. An undefined callee traps the
/// interpreter (`Trap::NoSuchFunction`) on both sides, and `same` passes a
/// two-sided trap silently — the case would compile, execute nothing, and prove
/// nothing. The definition also gives the loop a real call, which is what puts
/// the accumulator under the callee-saved ceiling and makes the header residency
/// worth anything at all.
#[test]
fn residency_carries_across_the_back_edge() {
    // A value used every iteration, register-held at the latch, must be marked
    // register-resident at the loop header once the fixpoint converges — not
    // reloaded fresh each iteration. Meaning must be preserved regardless.
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int s=0,i;for(i=0;i<20;i++)s+=e(i)+p;return s+p;} int main(void){return hot(3);}",
        "int e(int x){return x*3+1;} int hot(int p,int q){int s=0,i;for(i=0;i<15;i++)s+=e(i)*p+q;return s;} int main(void){return hot(2,5);}",
    ]);
}

/// R4.2 — an ABI-boundary truncation is a no-op, and it is GONE.
///
/// AAPCS64 §6.4.2/§6.8.2 leave the bits above an argument's or a result's
/// declared width unspecified, so a narrow self-move into a fixed argument
/// register, or out of a fixed result register, truncates something no
/// conforming program observes. Two shapes are pinned, both by COUNTING on the
/// physical MIR — the bisimulation above already says the function still
/// computes the right thing, so what is left to show is that the instructions
/// really left:
///
///   * `mov x16, xN ; mov wN, w16` — the windmill breaking a one-element
///     "cycle" that was only ever an identity assignment. The scratch register
///     is reserved for real cycles; a call-argument setup must never touch it.
///   * `mov wN, wN` — the standalone self-move, at a call boundary.
///
/// The program is chosen so the allocator has every reason to colour the
/// argument temps into x0-x7 directly: several `int` calls in a row whose
/// arguments are already the values the callee wants.
#[test]
fn abi_boundary_truncation_leaves_no_instruction() {
    let src = "int h(int,int,int);\n\
               int main(void){\n\
               int a=3,b=5,c=7,s=0;\n\
               s+=h(a,b,c); s+=h(b,c,a); s+=h(c,a,b);\n\
               s+=h(s,a,b); s+=h(a,s,c);\n\
               return s&0xff;\n}\n";
    same(src);

    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let p = crate::compile::backend(&h).unwrap();
    let f = p.funcs.iter().find(|f| f.name == "main").unwrap();
    use crate::mir::{isa, MInst, Reg, Width};
    let (mut scratch, mut selfmove) = (0usize, 0usize);
    for inst in f.blocks.iter().flat_map(|b| b.insts.iter()) {
        if let MInst::Copy { w, dst, src } = inst {
            if *dst == Reg::P(isa::SCRATCH_GPR) {
                scratch += 1;
            }
            if dst == src && *w != Width::W64 {
                selfmove += 1;
            }
        }
    }
    assert_eq!(
        scratch, 0,
        "x16 was used to break a cycle that is an identity assignment"
    );
    assert_eq!(selfmove, 0, "a narrow self-move survived at an ABI boundary");
}

/// R4.2 FPR TWIN — the same no-op, one register class over. A `double` carried
/// across a loop back edge becomes an `fmov dN, dN` edge copy the moment biased
/// colouring lands parameter and argument on the same register; the windmill
/// reads that identity as a one-element cycle and routes it through the reserved
/// v31 (`fmov d31, dN ; fmov dN, d31`), two instructions where the answer is
/// none (779 pairs on sqlite). It is a no-op because the pair's OWN width is `d`:
/// a 128-bit value would carry `Width::Q`, so at `s`/`d` no `q`-form reader can
/// observe the bits `fmov d,d` would zero (AAPCS64 §6.8.2). A GENUINE swap of two
/// doubles still needs the scratch, so the test both counts identity self-moves
/// to zero AND checks the differential where a real cycle survives.
#[test]
fn a_double_self_move_across_an_edge_leaves_no_instruction() {
    // two double accumulators carried around a loop: an edge copy per iteration,
    // but no genuine cycle, so nothing should reach v31.
    let src = "double f(int n){\n\
               double s=0.0, t=1.0;\n\
               for(int i=0;i<n;i++){ s+=t; t*=1.5; }\n\
               return s+t;\n}\n\
               int main(void){ return (int)f(6) & 0xff; }\n";
    same(src);

    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let p = crate::compile::backend(&h).unwrap();
    let f = p.funcs.iter().find(|f| f.name == "f").unwrap();
    use crate::mir::{isa, MInst, Reg};
    let scratch = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, MInst::FMov { dst, .. } if *dst == Reg::P(isa::SCRATCH_FPR)))
        .count();
    assert_eq!(
        scratch, 0,
        "v31 was used to break a `double` edge copy that is an identity assignment"
    );

    // a REAL swap of two doubles is a genuine 2-cycle and MUST keep the scratch —
    // the differential is the whole assertion: drop it and the two values clobber.
    same("double f(double a,double b,int n){for(int i=0;i<n;i++){double u=a;a=b;b=u;}return a*10.0+b;}\
          int main(void){return (int)f(1.0,2.0,5) & 0xff;}");
}

/// …and the rule is NOT "drop every narrow self-move": one whose destination is
/// read wider than it was written still truncates, and deleting it would leave
/// the upper half of whatever the register held. This is the direction the
/// yarpgen s0188 miscompile came from — the value below is narrowed on an edge
/// and then read as a full 64-bit quantity in the successor.
#[test]
fn a_truncation_with_a_wide_reader_is_kept() {
    let src = "long g[8];\n\
               int main(void){\n\
               long l = g[0];\n\
               int t = (int)l;\n\
               long r = g[1] ? (long)t : l;\n\
               g[2] = r;\n\
               return (int)(r & 0xff);\n}\n";
    // the bisimulation IS the assertion here: if the truncation is dropped, the
    // two sides compute different values
    same(src);
}


/// R4-capstone (spec §4.1) — SSA RECONSTRUCTION AT A JOIN, the Braun-2013 half
/// of the restructure.
///
/// R4.1's carry crosses an edge only where EVERY predecessor holds the value
/// under one name — the dominance special case, which needs no phi precisely
/// because there is only one reaching definition. The general case has several:
/// the value sits in a register on one arm of a diamond and has to be reloaded on
/// the other, and the join cannot name either one. Braun 2013's answer is to give
/// the join a BLOCK PARAMETER and let each predecessor pass its own reaching
/// definition on its edge — which is what `reconstruct::insert_phi` builds.
///
/// The square it ships is structural, and that is not a weaker claim than the
/// interpreted one: a block parameter defined at the head and fed by every
/// predecessor IS an SSA phi, `destruct` already lowers it to parallel edge
/// copies, and `mir::verify` already re-derives "every use dominated by its
/// definition" from scratch. So the test asserts the shape — the join gained one
/// parameter, each predecessor's edge INTO the join gained exactly the register
/// it was asked to pass, and no other edge was touched — and the battery's
/// `same_all` above it holds the meaning of the diamonds this will be pointed at.
///
/// `insert_phi` has no caller in the compiler yet (the wiring is the next task),
/// so the meaning half cannot move: these two programs are the BEFORE reading,
/// recorded here so the task that wires it in changes the count and not the
/// answer.
#[test]
fn reconstruct_reconciles_a_join_with_a_phi() {
    // Meaning-preserving on real programs whose join reconstructs a value.
    // `e` is DEFINED: an undefined callee traps both interpreters and `same`
    // passes a two-sided trap in silence, which would prove nothing.
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int a; if(p>0){a=e(p);}else{a=e(-p);} return a+p+e(p);} int main(void){return hot(4);}",
        "int e(int x){return x*3+1;} int hot(int p){int a=e(p); if(p&1)a+=e(p+1); else a+=e(p+2); return a+e(p);} int main(void){return hot(7);}",
    ]);

    // EFFECT — a join whose two predecessors reach it with DIFFERENT registers:
    // b0 falls into b2 holding the value in a register it computed, b1 reaches
    // b2 with the same value reloaded out of a frame slot. No name spans both, so
    // b2 needs the parameter. The two predecessors also end in different
    // terminators (`cbz` and `b`), which is the part of `insert_phi` that has to
    // know every `MTarget` a terminator carries.
    use crate::mir::{Class, MFunc, MInst, MTarget, MTerm, Reg, RegSet, SlotKind, Width};
    let mut f = MFunc {
        name: "join".to_string(),
        blocks: Vec::new(),
        vregs: Vec::new(),
        slots: Vec::new(),
        entry: 0,
        is_static: false,
        is_weak: false,
        order: Vec::new(),
        laid_out: false,
        frame_size: 0,
        saved: RegSet::default(),
        dyn_stack: false,
        has_vla: false,
        outgoing: 0,
        fp_slot: 0,
        cs_saves: Vec::new(),
        physical: false,
    };
    let (b0, b1, b2) = (f.new_block(), f.new_block(), f.new_block());
    let slot = f.new_slot(4, 4, SlotKind::Spill);
    let in_reg = f.new_vreg(Width::W32);
    let reloaded = f.new_vreg(Width::W32);
    f.blocks[b0 as usize].insts.push(MInst::MovImm {
        w: Width::W32,
        dst: in_reg,
        imm: 7,
    });
    f.blocks[b0 as usize].term = MTerm::Cbz {
        w: Width::W32,
        reg: in_reg,
        zero: true,
        t: MTarget { block: b1, args: Vec::new() },
        f: MTarget { block: b2, args: Vec::new() },
    };
    f.blocks[b1 as usize].insts.push(MInst::Reload {
        slot,
        dst: reloaded,
        w: Width::W32,
    });
    f.blocks[b1 as usize].term = MTerm::B(MTarget { block: b2, args: Vec::new() });
    f.blocks[b2 as usize].term = MTerm::Ret;

    let p = super::reconstruct::insert_phi(
        &mut f,
        b2,
        Class::Gpr,
        Width::W32,
        &[(b0, in_reg), (b1, reloaded)],
    );

    assert_eq!(
        f.blocks[b2 as usize].params,
        vec![Reg::V(p)],
        "the join did not gain the block parameter that is the phi"
    );
    assert_eq!(f.vregs[p as usize].class, Class::Gpr);
    assert_eq!(f.vregs[p as usize].width, Width::W32);
    // b0 reaches the join on the FALL-THROUGH arm of a `cbz`, so the argument
    // must land on that target and on no other — an argument pushed onto the
    // wrong edge is a wrong value on every path through b1.
    match &f.blocks[b0 as usize].term {
        MTerm::Cbz { t, f: fl, .. } => {
            assert!(t.args.is_empty(), "the edge that does NOT reach the join gained an argument");
            assert_eq!(fl.args, vec![in_reg], "the register-resident arm passes its register");
        }
        other => panic!("terminator rewritten: {:?}", other),
    }
    match &f.blocks[b1 as usize].term {
        MTerm::B(t) => assert_eq!(t.args, vec![reloaded], "the reloaded arm passes its reload"),
        other => panic!("terminator rewritten: {:?}", other),
    }
    // and the parameter is a FRESH name, not one of the two it reconciles
    assert!(p != in_reg.vreg().unwrap() && p != reloaded.vreg().unwrap());
}


/// R4-capstone (spec §4.1) — GENERALIZED CROSS-EDGE CARRY, the step that turns
/// `reconstruct::insert_phi` from a proven object into a working part of the
/// allocator.
///
/// R4.1's carry crosses an edge only where every predecessor holds the value
/// under ONE name. The switch below is the shape that condition throws away: five
/// arms that each reload the same variable arrive at the join holding five
/// DIFFERENT copies of it, so the value is in a register on every single path
/// and the join reloads it anyway — the dominance test cannot see a value that is
/// everywhere under five names. Braun 2013's block parameter can: each arm passes
/// its own copy on its own edge.
///
/// THE MEANING half is the battery's ordinary obligation, ⟦mir_v⟧ = ⟦mir_p⟧ over
/// a program with a real reconstruction in it. THE EFFECT half is what stops the
/// square from being vacuous (Law 0, spec §5): the SAME program is allocated
/// twice, once with the reconstruction and once without (`set_reconstruct`), and
/// the reload count must fall. A square that holds because nothing happened is
/// not a proof of anything.
///
/// TWO THINGS ABOUT THE PROGRAM, both of them measurements rather than taste.
/// `e` is DEFINED, because an undefined callee traps both interpreters and
/// `same` passes a two-sided trap in silence — the case would compile, execute
/// nothing and prove nothing. It is defined RECURSIVELY, because a small
/// straight-line `e` is inlined away and the call is exactly what creates the
/// pressure this fixture is about: with an inlinable `e` the same program spills
/// 23 times instead of 101 and the effect it is meant to show shrinks to noise.
/// The 24 loop-invariant values are there for the same reason — the ceiling that
/// forces the spilling is the callee-saved count, and nothing below it spills at
/// all.
///
/// `promote` runs on BOTH sides, so the differential is measured against an
/// allocator that has already rescued everything a wholly-free callee-saved
/// register could hold (R4.16). What is left is what only reconstruction reaches.
#[test]
fn generalized_carry_cuts_switch_reloads() {
    // Meaning: the shape, on a program small enough to read.
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int s=0,i; for(i=0;i<40;i++){switch(i%5){case 0:s+=e(i)+p;break;case 1:s+=e(i)*p;break;case 2:s+=p-e(i);break;case 3:s+=e(i)+p+p;break;default:s+=e(i);}} return s+p;} int main(void){return hot(3);}",
    ]);

    let n = 24;
    let decls: String = (0..n)
        .map(|i| format!("int v{}=g[{}]*{}+p;", i, i, i + 2))
        .collect();
    let sum: String = (0..n).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int e(int x){{return x<=0?1:e(x-1)+3;}} int g[64];\n\
         int hot(int p){{ int i,s=0; {d}\n\
         for(i=0;i<40;i++){{ switch(i%5){{\
         case 0: s+=e(i){s};break;\
         case 1: s+=e(i)*p{s};break;\
         case 2: s+=p-e(i){s};break;\
         case 3: s+=e(i)+p+p{s};break;\
         default: s+=e(i){s};}} }}\n\
         return s{s}; }}\n\
         int main(void){{ return hot(3); }}\n",
        d = decls,
        s = sum
    );

    // the meaning obligation holds on the pressure-heavy program too — it is the
    // one the effect is measured on, so it is the one the square has to cover
    same(&src);

    let reloads = |level: u8| -> usize {
        super::spill::set_reconstruct(level);
        let ast = frontend(&src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let p = crate::compile::backend(&h).unwrap();
        super::spill::set_reconstruct(super::spill::RECON_LOOPS);
        super::promote::set_enabled(true);
        let f = p.funcs.iter().find(|f| f.name == "hot").unwrap();
        f.blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|i| matches!(i, crate::mir::MInst::Reload { .. }))
            .count()
    };
    let off = reloads(super::spill::RECON_NONE);
    let on = reloads(super::spill::RECON_JOINS);
    assert!(
        off > 0,
        "the fixture does not spill at all — the square below it would be vacuous"
    );
    assert!(
        on < off,
        "reconstruction removed no reload: {} with it, {} without",
        on,
        off
    );
}


/// R4-capstone (spec §4.2) — THE LOOP-HEADER CARRY, and what it is actually
/// made of.
///
/// The back-edge fixpoint shipped dark one step ago, and flipping it on is worth
/// EXACTLY NOTHING by itself: a header's carry used to be an intersection over
/// its predecessors, and a preheader holds nothing under a latch's name, so the
/// intersection was empty every time it was asked. The step is the PHI. The
/// header takes a block parameter, the latch feeds it the register it is still
/// holding, and the preheader feeds it one reload — paid once, against a reload
/// the body was paying every iteration. The fixpoint is what lets the header
/// know what the latch will hold, one round behind; it is the mechanism, not the
/// step.
///
/// WHAT THIS CAN AND CANNOT CARRY, measured rather than hoped. A value whose
/// block PARAMETER the spiller evicted has no definition left in the IR — its
/// definition is the store each edge now makes into its slot — and a register
/// copy of such a name goes stale the moment an edge writes the slot. Carrying
/// one around a back edge hands the next iteration the previous iteration's
/// value, induction variable included, and the loop never ends; that is a real
/// defect this battery caught (see `has_def` in `spill.rs`). So what a header
/// phi carries is a value with a definition that dominates the loop — a
/// LOOP-INVARIANT spilled value, reloaded every iteration today because
/// `Sim::More` sends a value to memory for its whole life however cheap the loop
/// it is read in. That is R4.16's region-residency, generalized from "a wholly
/// free register exists" to "a register is free HERE".
///
/// The fixture is that shape: 24 values live across a high-pressure statement,
/// which sends them all to memory, and then a low-pressure loop that reads five
/// of them. The A/B is `RECON_JOINS` against `RECON_LOOPS`, so the number
/// belongs to this step alone and not to §4.1's joins. `e` is defined (an
/// undefined callee traps both interpreters and `same` passes a two-sided trap
/// in silence) and defined RECURSIVELY (an inlinable `e` removes the call, and
/// the call is what creates the pressure).
#[test]
fn loop_header_carry_keeps_the_accumulator_in_a_register() {
    // Meaning: the brief's loop-carried programs, small enough to read.
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int acc=0,i; for(i=0;i<50;i++){acc=acc+e(i)+p;} return acc; } int main(void){return hot(2);}",
        "int e(int x){return x*3+1;} int hot(int p){int a=0,b=0,i; for(i=0;i<30;i++){a+=e(i)*p; b+=e(i)+a;} return a+b; } int main(void){return hot(3);}",
    ]);

    let n = 24;
    let decls: String = (0..n)
        .map(|i| format!("int v{}=g[{}]*{}+p;", i, i, i + 2))
        .collect();
    let sum: String = (0..n).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int e(int x){{return x<=0?1:e(x-1)+3;}} int g[64];\n\
         int hot(int p){{ int i,s=0; {d}\n\
         s = 0{s};\n\
         for(i=0;i<20;i++){{ s += e(i)+v0+v1+v2+v3+v4; }}\n\
         return s{s}; }}\n\
         int main(void){{ return hot(3); }}\n",
        d = decls,
        s = sum
    );
    same(&src);

    let count = |level: u8| -> (usize, usize) {
        // The measurement is about the SPILLER's carry, so the promotion pass —
        // which runs after it and would remove the same frame traffic by another
        // route — is out of the way for it.
        super::promote::set_enabled(false);
        super::spill::set_reconstruct(level);
        let ast = frontend(&src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let p = crate::compile::backend(&h).unwrap();
        super::spill::set_reconstruct(super::spill::RECON_LOOPS);
        super::promote::set_enabled(true);
        let f = p.funcs.iter().find(|f| f.name == "hot").unwrap();
        use crate::mir::MInst;
        let it = || f.blocks.iter().flat_map(|b| b.insts.iter());
        (
            it().filter(|i| matches!(i, MInst::Reload { .. })).count(),
            it().filter(|i| matches!(i, MInst::Spill { .. })).count(),
        )
    };
    let joins = count(super::spill::RECON_JOINS);
    let loops = count(super::spill::RECON_LOOPS);
    assert!(
        joins.0 > 0,
        "the fixture does not spill at all — the square above it would be vacuous"
    );
    assert!(
        loops.0 < joins.0 && loops.1 <= joins.1,
        "the loop-header carry removed no frame traffic: {:?} with it, {:?} with joins only",
        loops,
        joins
    );
}


/// R4-capstone (spec §4.3) — EVICTION IS A REGIONAL SPLIT, not a whole-web spill.
///
/// `Sim::More` used to be a life sentence. One pressure peak anywhere in a
/// function sent a value to memory for the whole of its live range, and every
/// later read of it — in blocks with registers to spare, in loops it was merely
/// passing through — paid a frame load. Braun & Hack's algorithm does not say
/// that; it says a value leaves the register file WHERE pressure forces it out
/// and comes back at its next use. The regions in between are register-resident,
/// and the register they are resident in is the value's OWN name.
///
/// The measurement that sized this before a line of it was written is the
/// `split` column of `ZCC_SPILLCEIL` (`spill.rs::ceiling_report`): on sqlite3.c,
/// 2,084 of 11,520 reloads are of a value some predecessor was still holding in
/// a register, in a block whose head had registers to spare — and 4,370 of the
/// 4,549 values this allocator sends to memory hold a register SOMEWHERE in the
/// function against 179 that hold one nowhere. The whole-web model is wrong for
/// 96% of them, and those 179 are the ones it is right about: a value that can
/// hold no register anywhere still becomes memory-resident for its whole life,
/// which is what keeps the memory lattice growing and the fixpoint bounded.
///
/// THE MEANING half is the battery's ordinary obligation on the brief's own
/// program — a value read either side of a long call chain — with `e` DEFINED,
/// since an undefined callee traps both interpreters and `same` accepts a
/// two-sided trap in silence.
///
/// THE EFFECT half needs a program with EDGES in it, and the brief's does not
/// have any: the split of a straight-line value is what the eviction path
/// already did before this step ((2c) in `simulate` never filtered on `spilled`),
/// and what §4.3 adds is that the split SURVIVES A BLOCK BOUNDARY. So the
/// differential is measured on eight arms reading twenty-four memory-resident
/// values, `set_regional(false)` against `set_regional(true)`.
///
/// WHAT IS ASSERTED, and why it is the brief's assertion in the form this layer
/// can see it. The brief asks that the value not be whole-web memory-resident —
/// that it have a register-resident interval. `Plan.wexit` is gone by the time a
/// test can look, and the allocated function has no virtual registers left in it
/// to ask about; what survives is the pair (still spilled, fewer reloads). A
/// value with a `Spill` is memory-resident somewhere by construction, and a
/// strictly smaller reload count over the same program with the same spills is
/// exactly a read served from a register that used to be served from the frame.
/// The instruction count is asserted with it, because a reload traded for two
/// copies is not a win and the cost square is the half a reload count cannot see.
#[test]
fn eviction_splits_regionally_not_whole_web() {
    // Meaning: the brief's program, `e` defined so both interpreters run it.
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int a=e(p); int t=e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a); int b=a+t+p; return b+a; } int main(void){return hot(2);}",
    ]);

    let n = 24;
    let decls: String = (0..n)
        .map(|i| format!("int v{}=g[{}]*{}+p;", i, i, i + 2))
        .collect();
    let sum: String = (0..n).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int e(int x){{return x<=0?1:e(x-1)+3;}} int g[64];\n\
         int hot(int p){{ int i,s=0; {d}\n\
         s = 0{s};\n\
         if(p>0){{ s += v0+v1+v2; }} else {{ s -= v3+v4+v5; }}\n\
         if(p>1){{ s += v6+v7+v8; }} else {{ s -= v9+v10+v11; }}\n\
         if(p>2){{ s += v12+v13+v14; }} else {{ s -= v15+v16+v17; }}\n\
         if(p>3){{ s += v18+v19+v20; }} else {{ s -= v21+v22+v23; }}\n\
         s += e(s);\n\
         if(p>1){{ s += v0+v3+v6; }}\n\
         for(i=0;i<20;i++){{ s += e(i)+v0+v1+v2+v3+v4; }}\n\
         return s{s}; }}\n\
         int main(void){{ return hot(3); }}\n",
        d = decls,
        s = sum
    );
    // the meaning obligation holds on the program the effect is measured on too
    same(&src);

    let count = |on: bool| -> (usize, usize, usize) {
        super::spill::set_regional(on);
        let ast = frontend(&src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let p = crate::compile::backend(&h).unwrap();
        super::spill::set_regional(true);
        let f = p.funcs.iter().find(|f| f.name == "hot").unwrap();
        use crate::mir::MInst;
        let it = || f.blocks.iter().flat_map(|b| b.insts.iter());
        (
            it().filter(|i| matches!(i, MInst::Reload { .. })).count(),
            it().filter(|i| matches!(i, MInst::Spill { .. })).count(),
            it().count(),
        )
    };
    let whole = count(false);
    let split = count(true);
    assert!(
        whole.0 > 0 && split.1 > 0,
        "the fixture does not spill at all — the square above it would be vacuous: \
         whole-web {:?}, regional {:?}",
        whole,
        split
    );
    assert!(
        split.0 < whole.0,
        "regional eviction served no read from a register: {} reloads split, {} whole-web",
        split.0,
        whole.0
    );
    assert!(
        split.2 < whole.2,
        "regional eviction removed reloads but not instructions — the reads it saved \
         were paid for in copies: {} instructions split, {} whole-web",
        split.2,
        whole.2
    );
}


/// R4-capstone (spec §6, the "block-param explosion" risk) — PRUNING, and the
/// two questions a pruning pass has to answer that a correctness square cannot.
///
/// Prune nothing and the program is still right, only bigger; so "does it hold"
/// is the wrong question and "what does it remove, and does it remove anything
/// it should have kept" is the right one. Both are measured here against
/// `set_prune(false)`.
///
/// WHAT IS PRUNED. Two things, and the second is the one that carries the
/// numbers. A parameter no use reads is dropped — decided at a block head where
/// "the value is read at or below here" is the best that can be said, and the
/// walk may then evict it before that read. And a parameter every incoming edge
/// reaches with the SAME reaching definition is not a parameter at all: it IS
/// that definition (Braun 2013 §2.3, `removeTrivialPhi`, with a loop header's
/// self-reference discounted so the fixpoint can see through a cycle). Keeping
/// one costs a parallel copy on every edge — `destruct` emits one per edge and
/// only biased colouring removes any of them — and buys nothing. On sqlite3.c
/// **1,883 of 2,839 parameters are trivial in that sense**, worth 530 static
/// instructions and 496 `mov`s with frame traffic unchanged.
///
/// WHAT IS ASSERTED, and why it is the brief's assertion in the form this layer
/// can see it. The brief asks that every parameter inserted remove at least one
/// reload. A parameter is gone by the time a test can look — `destruct` has
/// lowered it to edge copies — so the observable statement of "it removed no
/// reload" is that REMOVING IT COSTS NO RELOAD: pruning may only take away
/// parameters whose absence nothing has to reload for. So reloads must not rise
/// and instructions must fall. The other half of the brief, that colouring never
/// exceeds k, is checked by the pipeline itself on every one of these programs —
/// `spill::check_pressure` and `mir::verify` run inside `compile::backend`, and
/// `regalloc::verify` runs in `same` — which is why a passing compile IS that
/// assertion and no separate one is written.
#[test]
fn reconstruction_is_pruned_and_pressure_is_counted() {
    // Meaning on nested loops + wide joins, `e` defined so both interpreters
    // actually run it (an undefined callee traps both and `same` accepts a
    // two-sided trap in silence).
    same_all(&[
        "int e(int x){return x*3+1;} int hot(int p){int s=0,i,j; for(i=0;i<10;i++)for(j=0;j<10;j++){switch((i+j)%4){case 0:s+=e(i)+p;break;case 1:s+=e(j)*p;break;case 2:s+=p;break;default:s+=e(i*j);}} return s+p;} int main(void){return hot(2);}",
    ]);

    let n = 24;
    let decls: String = (0..n)
        .map(|i| format!("int v{}=g[{}]*{}+p;", i, i, i + 2))
        .collect();
    let sum: String = (0..n).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int e(int x){{return x<=0?1:e(x-1)+3;}} int g[64];\n\
         int hot(int p){{ int i,j,k,s=0; {d}\n\
         s = 0{s};\n\
         for(i=0;i<10;i++){{ s += e(i)+v0+v1+v2;\n\
           for(j=0;j<10;j++){{ s += e(j)+v3+v4+v5+v6;\n\
             for(k=0;k<10;k++){{ s += e(k)+v7+v8+v9+v10+v11; }}\n\
             s += v12+v13; }}\n\
           s += v14+v15; }}\n\
         return s{s}; }}\n\
         int main(void){{ return hot(3); }}\n",
        d = decls,
        s = sum
    );
    same(&src);

    let count = |on: bool| -> (usize, usize, usize) {
        super::spill::set_prune(on);
        let ast = frontend(&src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let p = crate::compile::backend(&h).unwrap();
        super::spill::set_prune(true);
        let f = p.funcs.iter().find(|f| f.name == "hot").unwrap();
        use crate::mir::MInst;
        let it = || f.blocks.iter().flat_map(|b| b.insts.iter());
        (
            it().filter(|i| matches!(i, MInst::Reload { .. })).count(),
            it().filter(|i| matches!(i, MInst::Spill { .. })).count(),
            it().count(),
        )
    };
    let kept = count(false);
    let pruned = count(true);
    assert!(
        kept.0 > 0,
        "the fixture does not spill at all — the square above it would be vacuous"
    );
    assert!(
        pruned.2 < kept.2,
        "pruning removed no instruction — either no parameter was dead or trivial, \
         or the prune is not firing: {} instructions pruned, {} kept",
        pruned.2,
        kept.2
    );
    assert!(
        pruned.0 <= kept.0,
        "pruning removed a parameter that was removing a reload: {} reloads pruned, \
         {} kept",
        pruned.0,
        kept.0
    );
}


/// R4-capstone (spec §4.4) — THE CARRY BUDGET AT LOOP DEPTH ≥ 2.
///
/// Termination no longer waits for the register-residency lattice to reach a
/// fixed point (it oscillates: 113,024 rounds on one sqlite function, measured).
/// It spends a BUDGET of re-seeding rounds instead, and the budget is the
/// function's loop nesting depth plus one — the claim being that one round is
/// what it takes for a latch's residency to become visible to its own header, so
/// a value carried around an inner loop reaches the ENCLOSING header one round
/// later and one round per level sees all of them.
///
/// That is a structural argument about a number, and until this test nothing
/// exercised it above depth 1 — the whole ladder rested on a bound no fixture
/// had ever pushed. The program below is doubly nested with loop-invariant
/// values read in both bodies, so if the budget were short by one level the
/// OUTER header would never learn what its latch holds and its parameters would
/// not exist. The assertion is on the tally of parameters at loop headers
/// (`take_phi_tally`): §4.2's carry has to fire more than once, which at one
/// nesting level per round it cannot do unless the budget really is depth + 1.
#[test]
fn the_carry_budget_reaches_a_doubly_nested_header() {
    let n = 24;
    let decls: String = (0..n)
        .map(|i| format!("int v{}=g[{}]*{}+p;", i, i, i + 2))
        .collect();
    let sum: String = (0..n).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int e(int x){{return x<=0?1:e(x-1)+3;}} int g[64];\n\
         int hot(int p){{ int i,j,s=0; {d}\n\
         s = 0{s};\n\
         for(i=0;i<10;i++){{ s += e(i)+v0+v1+v2;\n\
           for(j=0;j<10;j++){{ s += e(j)+v3+v4+v5+v6; }}\n\
           s += v7+v8; }}\n\
         return s{s}; }}\n\
         int main(void){{ return hot(3); }}\n",
        d = decls,
        s = sum
    );
    same(&src);

    // The tally is taken with the PRUNING OFF, because the question is what the
    // carry DECIDED, not what survived: a header parameter every edge reaches
    // with one name is trivial and is removed, and counting only the survivors
    // would read a successful carry as a carry that never happened.
    let tally = |level: u8| -> (usize, usize) {
        super::spill::set_reconstruct(level);
        super::spill::set_prune(false);
        let _ = super::spill::take_phi_tally();
        let ast = frontend(&src);
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        let _ = crate::compile::backend(&h).unwrap();
        super::spill::set_reconstruct(super::spill::RECON_LOOPS);
        super::promote::set_enabled(true);
        super::spill::set_prune(true);
        super::spill::take_phi_tally()
    };
    let joins = tally(super::spill::RECON_JOINS);
    let loops = tally(super::spill::RECON_LOOPS);
    assert_eq!(
        joins.1, 0,
        "a loop-header parameter was built with §4.2 switched off: {:?}",
        joins
    );
    assert!(
        loops.1 >= 2,
        "the carry reached only {} loop header(s) on a doubly-nested function — \
         the budget of loop-nesting-depth + 1 re-seeding rounds is not reaching \
         the outer header: {:?} with §4.2, {:?} with joins only",
        loops.1,
        loops,
        joins
    );
}


/// AN EDGE IS A PARALLEL COPY, AND A SPILL SLOT IS ONE OF ITS LOCATIONS.
///
/// `evict_params` removes a spilled block parameter and makes each predecessor
/// store the value it would have passed straight into the parameter's slot. When
/// ANOTHER argument on that same edge is itself resident in that slot, the edge
/// both reads and writes one location, and read-before-write — the defining
/// property of a parallel copy — has to hold across the register/slot boundary,
/// not just between registers.
///
/// It did not. A rotation of three pointers across a loop back edge
/// (`t=pt[0]; pt[0]=pt[1]; pt[1]=pt[2]; pt[2]=t;`) under enough register
/// pressure to spill emitted
///
///     str x13, [sp, #88]        // pt[2]' = pt[0]   — writes the slot
///     ldr x13, [sp, #88]        // pt[1]' = pt[2]   — reads what it just wrote
///
/// so the rotation lost a pointer. sqlite's `wherePathSolver` chooses its join
/// order with exactly that rotation, so a two-table query built a plan naming a
/// cursor that was never opened and `sqlite3VdbeExec` dereferenced NULL: the
/// zcc-built sqlite CLI SIGSEGV'd on any join. The fix materializes the read
/// into a fresh value before the stores run, which is the move `seq_copy`
/// already makes for a register cycle, with a slot as the location.
///
/// The pressure matters — with few enough live values nothing spills, no
/// parameter is evicted, and the hazard cannot arise — so the fixture carries
/// twelve loop-carried accumulators alongside the rotation.
#[test]
fn an_edge_reads_a_slot_before_the_edge_overwrites_it() {
    same(
        "int main(void){\n\
           static int M[4][32];\n\
           int *pt[3];\n\
           pt[0]=M[0]; pt[1]=M[1]; pt[2]=M[2];\n\
           long v0=5,v1=4,v2=2,v3=2,v6=6,v7=5,v8=6,v9=7;\n\
           long v12=4,v13=8,v14=2,v15=2,v16=7,v17=7;\n\
           int i, j; long s = 0;\n\
           for(i=0;i<17;i++){\n\
             int *t;\n\
             v1=(v1^(i+1))&0xffff; v2=(v2*(i+4))&0xffff; v3=(v3^(i+2))&0xffff;\n\
             v8=(v8^(i+3))&0xffff; v15=(v15*(i+1))&0xffff;\n\
             v16=(v16^(i+2))&0xffff; v17=(v17^(i+5))&0xffff;\n\
             for(j=0;j<32;j++){ pt[0][j] = (int)(v13 + j); }\n\
             t=pt[0]; pt[0]=pt[1]; pt[1]=pt[2]; pt[2]=t;\n\
             s += v0 + pt[0][i%32]; s += v1 + pt[1][i%32];\n\
             s += v6 + pt[0][i%32]; s += v7 + pt[1][i%32];\n\
             s += v9 + pt[0][i%32]; s += v12 + pt[0][i%32];\n\
             s += v13 + pt[1][i%32]; s += v14 + pt[2][i%32];\n\
           }\n\
           return (int)(s & 0x7fffffff);\n\
         }",
    );
}

/// LAW-4 EXHAUSTION FOR THE EDGE-AS-PARALLEL-COPY THEOREM.
///
/// `an_edge_reads_a_slot_before_the_edge_overwrites_it` pins ONE shape, and one
/// shape is where a proof stops being a proof and becomes an anecdote — the
/// defect it names shipped through a 20,000-program generated seal, a 1,694-case
/// torture suite and a 1,552-case opt-parity run, because none of them held a
/// rotation under enough register pressure to evict a parameter.
///
/// The family is generated instead of hand-picked: a permutation of K pointers
/// carried around a loop, under P loop-carried accumulators. K decides how many
/// locations the edge's parallel copy has to move at once, P decides whether the
/// allocator runs out of registers and starts evicting parameters into slots —
/// and the defect needs BOTH, which is why every fixture written by hand at one
/// pressure level missed it. Each member is checked by the allocator's commuting
/// square `⟦mir_v⟧ = ⟦mir_p⟧`, so a failure names the shape rather than a
/// checksum.
#[test]
fn every_pointer_rotation_under_pressure_keeps_its_permutation() {
    for k in 2..=4usize {
        for p in [6usize, 10, 14] {
            let mut s = String::from("int main(void){\n static int M[5][32];\n int *pt[");
            s.push_str(&format!("{}];\n", k));
            for i in 0..k {
                s.push_str(&format!(" pt[{}]=M[{}];\n", i, i));
            }
            for i in 0..p {
                s.push_str(&format!(" long v{}={};\n", i, i % 7 + 2));
            }
            s.push_str(" int i,j; long acc=0;\n for(i=0;i<13;i++){\n  int *t;\n");
            for i in 0..p {
                let op = ["^", "*", "+"][i % 3];
                s.push_str(&format!("  v{}=(v{} {} (i+{}))&0xffff;\n", i, i, op, i % 5 + 1));
            }
            s.push_str("  for(j=0;j<32;j++){ pt[0][j]=(int)(v0+j); }\n");
            // the rotation: t = pt[0]; pt[0] = pt[1]; ... ; pt[k-1] = t
            s.push_str("  t=pt[0];\n");
            for i in 0..k - 1 {
                s.push_str(&format!("  pt[{}]=pt[{}];\n", i, i + 1));
            }
            s.push_str(&format!("  pt[{}]=t;\n", k - 1));
            for i in 0..p {
                s.push_str(&format!("  acc+=v{}+pt[{}][i%32];\n", i, i % k));
            }
            s.push_str(" }\n return (int)(acc & 0x7fffffff);\n}");
            same(&s);
        }
    }
}

/// MECHANISM.md Part D S1 — BELADY'S DISTANCE IS MEASURED ALONG THE TRACE, NOT THE TEXT.
///
/// `linear_positions` numbers instructions in reverse postorder, in which a back
/// edge runs backwards. A value carried around a loop is therefore read at a
/// LOWER position than the latch that passes it on, so the static `next_use`
/// from the latch found nothing and answered "never used again" — the strongest
/// possible reason to evict. Measured on `tests/bench/nestjoin.c`: the loop
/// index, the loop pointer and the accumulator were all spilled out of a
/// four-million-iteration inner loop, while twenty-four values used only after
/// both loops kept their registers. Six of the inner loop's eleven instructions
/// were frame traffic; removing them by hand made the program as fast as
/// `gcc -O1` (8 ms → 1 ms), which is the whole reason this row exists.
///
/// The fixture is that shape: `n` values live ACROSS an inner loop but read only
/// after it, against three values the inner loop reads every iteration. The
/// theorem says the three win, so the frame traffic left inside the inner loop
/// is bounded by what the model still cannot place — the invariant pointer's
/// reload and the accumulator's live-out store, both S2's business, not S1's.
///
/// NON-VACUOUS: with the distance taken statically — the one-line change of
/// asking `next_use` instead of the trace — the same fixture leaves FOUR frame
/// instructions in that block and this test fails.
#[test]
fn eviction_ranks_by_dynamic_distance_not_text_order() {
    let n = 24;
    let decls: String = (0..n).map(|i| format!("long c{}=pa[{}];", i, i)).collect();
    let bump: String = (0..n).map(|i| format!("c{}+=i;", i)).collect();
    let sum: String = (0..n).map(|i| format!("+c{}", i)).collect();
    let src = format!(
        "long joinit(int *pa, int *pb, int n, int m){{\n\
         {d} long hits=0; int i,j;\n\
         for(i=0;i<n;i++){{ int key=pa[i];\n\
         for(j=0;j<m;j++){{ if(pb[j]==key) hits++; }}\n\
         {b} }}\n\
         return hits{s}; }}\n\
         int A[64],B[64];\n\
         int main(void){{ int k; for(k=0;k<64;k++){{A[k]=k%7;B[k]=k%7;}}\n\
         return (int)joinit(A,B,64,64); }}\n",
        d = decls,
        b = bump,
        s = sum
    );
    same(&src);

    let ast = frontend(&src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let p = crate::compile::backend(&h).unwrap();
    let f = p.funcs.iter().find(|f| f.name == "joinit").unwrap();
    let cfg = crate::mir::verify::cfg(f);
    let dt = crate::cfg::DomTree::new(&cfg, f.entry);
    let lf = crate::cfg::LoopForest::new(&cfg, &dt);
    let deepest = *lf.depth.iter().max().unwrap();
    assert!(deepest >= 2, "the fixture lost its inner loop; nothing is being tested");
    use crate::mir::MInst;
    let count = |f: &crate::mir::MFunc, pred: fn(&MInst) -> bool| -> usize {
        (0..f.blocks.len())
            .filter(|&b| lf.depth[b] == deepest)
            .map(|b| f.blocks[b].insts.iter().filter(|i| pred(i)).count())
            .sum()
    };
    let reloads = count(f, |i| matches!(i, MInst::Reload { .. }));
    let traffic = reloads + count(f, |i| matches!(i, MInst::Spill { .. }));
    // A RELOAD in the innermost loop is the expensive kind: it stands in front of
    // the use, so it lengthens the dependence chain the loop runs four million
    // times (Law 3c). The loop's pointer is invariant — one reload before the
    // loop serves every iteration — and refusing it a register cost `nestjoin.c`
    // its whole remaining gap: 12 ms against gcc's 11 on the 36-million-iteration
    // form, 11 ms once the phi that carries it was allowed to exist.
    assert_eq!(
        reloads, 0,
        "the innermost loop reloads {} value(s) every iteration",
        reloads
    );
    assert!(
        traffic <= 1,
        "the innermost loop carries {} frame instructions: the hot values lost to the cold ones again",
        traffic
    );
}

/// R5.1 — THE WEIGHTS DECIDE SOMETHING, AND WHAT THEY DECIDE IS STILL RIGHT.
///
/// The row has three parts and one switch: `freq::annotate` computes the block
/// frequencies, `layout` chains blocks so the heavy edge falls through, and the
/// spiller's `Trace::rank` scales Belady's distance by the frequency of the block
/// where the reload would be paid. A commuting square proves nothing where the
/// pass never fires, so this asks both halves of the obligation:
///
///   * NON-VACUITY — the emitted text differs with the weights on. The fixture
///     is built to make it: twenty-four values that a call defines (so none can
///     be rematerialized), a hot inner loop that reads four of them, and a cold
///     guarded arm that reads the other twenty. Ranked by distance alone the two
///     groups are comparable; ranked by what an eviction COSTS they are not.
///   * MEANING — `⟦mir_v⟧ = ⟦mir_p⟧` with the weights on, and the same answer
///     with them off. A ranking may choose any victim it likes; it may not
///     change what the program computes.
#[test]
fn frequency_weights_move_the_code_and_not_the_answer() {
    let decls: String = (0..24).map(|i| format!("int v{}=f({});", i, i + 1)).collect();
    let cold: String = (4..24).map(|i| format!("+v{}", i)).collect();
    let src = format!(
        "int f(int x){{ return x*3+1; }}\n\
         int g(int n){{ int i, a = 0; {d}\n\
         for (i = 0; i < n; i++) {{ a += v0*v1 + v2*v3; a ^= a >> 3; }}\n\
         if (n < 0) {{ a += 0{c}; }}\n\
         return a + v0 + v23; }}\n\
         int main(void){{ return g(7); }}\n",
        d = decls,
        c = cold
    );
    let ast = frontend(&src);
    let build = |on: bool| -> (i64, i64, String) {
        hir::freq::set_weights(Some(on));
        let mut h = hir::build::build(&ast);
        hir::pass::run_module(&mut h);
        for f in h.funcs.iter_mut() {
            hir::freq::annotate(f);
        }
        let v = isel::lower(&h);
        let before = mi::new_machine(&v, &ast).call("main", &[], &[]).expect("⟦mir_v⟧ trapped");
        let p = crate::compile::backend(&h).expect("allocation failed");
        let after = mi::new_machine(&p, &ast).call("main", &[], &[]).expect("⟦mir_p⟧ trapped");
        let text = crate::emit::emit(&ast, &p);
        hir::freq::set_weights(None);
        (before as i32 as i64, after as i32 as i64, text)
    };
    let (voff, poff, off) = build(false);
    let (von, pon, on) = build(true);
    assert_eq!(voff, poff, "⟦mir_v⟧ != ⟦mir_p⟧ with the weights off");
    assert_eq!(von, pon, "⟦mir_v⟧ != ⟦mir_p⟧ with the weights on");
    assert_eq!(voff, von, "the weights changed the answer, which is not theirs to change");
    assert_ne!(
        off, on,
        "the weights changed nothing: the fixture stopped exercising layout and eviction"
    );
}
