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
    let p = crate::compile::backend(&h).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    for f in &p.funcs {
        super::verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
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
