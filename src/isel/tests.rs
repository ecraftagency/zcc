// The isel battery (REARCH.md §10 row "isel", §12 R0.6) — TRANSLATION
// VALIDATION, not a value table.
//
// The obligation of instruction selection is `⟦hir⟧ = ⟦mir_v⟧`: whatever the C
// program means, the machine sequence must mean the same. So this battery runs
// BOTH interpreters over the same function and compares — it never asserts an
// expected number, because that is `hir::tests`' job and duplicating it would
// let the two drift. A failure here localizes to one selection rule.
//
// Each program below is chosen to force a particular lowering rule; as R3.1
// adds munch patterns, a row is added here with it.
use super::lower;
use crate::hir::{self, interp as hi};
use crate::mir::{interp as mi, verify as mv};
use crate::testutil::frontend;

/// Run `main` through both layers and require agreement — once on raw HIR and
/// once on HIR after the §4 pass ladder, because a selection rule can be
/// unreachable until a pass folds its input into shape (the literal-address case
/// below is exactly that).
fn equiv(src: &str) {
    equiv_side(src, false);
    equiv_side(src, true);
}

fn equiv_side(src: &str, opt: bool) {
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    if opt {
        hir::pass::run_module(&mut h);
    }
    let h = h;
    for f in &h.funcs {
        hir::verify::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let m = lower(&h);
    for f in &m.funcs {
        mv::verify(f).unwrap_or_else(|e| panic!("{}\n{}", e, src));
    }
    let hv = {
        let mut mach = hi::new_machine(&h, &ast);
        mach.call("main", &[])
    };
    let mv_ = {
        let mut mach = mi::new_machine(&m, &ast);
        mach.call("main", &[], &[])
    };
    match (hv, mv_) {
        // ⟦hir⟧ carries an integer sign-extended; A64 zero-extends a `w` result.
        // Both are the same C value — compare at the return type, `int`.
        (Ok(a), Ok(b)) => {
            let (a, b) = (a.unwrap_or(0) as i32, b as i32);
            assert_eq!(a, b, "⟦hir⟧ = {} but ⟦mir⟧ = {}\n{}", a, b, src);
        }
        // A trap is ⊥ on both sides; the machine layer may refine it (A64 `sdiv`
        // by zero yields 0 where C says undefined), so only HIR trapping matters.
        (Err(_), _) => {}
        (Ok(a), Err(e)) => panic!("⟦mir⟧ trapped ({:?}) where ⟦hir⟧ = {:?}\n{}", e, a, src),
    }
}

fn equiv_all(cases: &[&str]) {
    for c in cases {
        equiv(c);
    }
}

#[test]
fn integer_alu() {
    equiv_all(&[
        "int main(void){return 42;}",
        "int main(void){int a=7,b=5;return a+b;}",
        "int main(void){int a=7,b=5;return a-b;}",
        "int main(void){int a=7,b=5;return a*b;}",
        "int main(void){int a=37,b=5;return a/b;}",
        "int main(void){int a=37,b=5;return a%b;}",
        "int main(void){unsigned a=37,b=5;return a/b;}",
        "int main(void){unsigned a=37,b=5;return a%b;}",
        "int main(void){int a=-37,b=5;return a/b;}",
        "int main(void){int a=-37,b=5;return a%b;}",
        "int main(void){int a=0xf0,b=0x3c;return (a&b)+(a|b)+(a^b);}",
        "int main(void){int a=1;return (a<<10)|(a<<3);}",
        "int main(void){int a=-64;return (a>>3);}",
        "int main(void){unsigned a=0xf0000000u;return a>>3;}",
        "int main(void){int a=5;return -a + ~a;}",
        "long main(void){long a=1;return (a<<40)+3;}",
    ]);
}

#[test]
fn immediate_legalization() {
    // Each constant lands in a different encoding class: imm12, imm12<<12, a
    // logical bitmask, a movz, a movn, and a full movz/movk chain.
    equiv_all(&[
        "int main(void){int a=1;return a+4095;}",
        "int main(void){int a=1;return a+4096;}",
        "int main(void){int a=1;return a+4097;}",
        "int main(void){int a=-1;return a&0xff;}",
        "int main(void){int a=-1;return a&0x1234;}",
        "long main(void){long a=1;return a|0x0f0f0f0f0f0f0f0fL;}",
        "int main(void){return -1;}",
        "long main(void){return 0x123456789abcdefL;}",
        "int main(void){int a=3;return a*1000000;}",
    ]);
}

#[test]
fn compares_and_branches() {
    equiv_all(&[
        "int main(void){int a=1,b=2;return (a<b)+(a<=b)+(a>b)+(a>=b)+(a==b)+(a!=b);}",
        "int main(void){unsigned a=1,b=2;return (a<b)+(a<=b)+(a>b)+(a>=b);}",
        "int main(void){int a=-1;unsigned b=1;return (unsigned)a>b;}",
        "int main(void){int a=3;if(a>2)return 10;else return 20;}",
        "int main(void){int s=0,i;for(i=0;i<10;i++)if(i&1)s+=i;return s;}",
        "int main(void){int a=0;return a?1:2;}",
        "int main(void){int a=5;while(a)a--;return a;}",
    ]);
}

#[test]
fn narrow_types_and_conversions() {
    equiv_all(&[
        "int main(void){char c=-3;return c;}",
        "int main(void){unsigned char c=253;return c;}",
        "int main(void){short s=-3;return s;}",
        "int main(void){unsigned short s=65533;return s;}",
        "int main(void){int x=300;char c=x;return c;}",
        "int main(void){char c=-3;return (int)c*2;}",
        "int main(void){char c=1;c++;c++;return c;}",
        "int main(void){long l=-1;return (int)l;}",
        "int main(void){unsigned u=0xffffffffu;return (long)u > 0;}",
        "int main(void){int i=-1;return (long)i < 0;}",
        "int main(void){_Bool b=5;return b;}",
        "int main(void){double d=0.0;_Bool b=d;return b;}",
    ]);
}

#[test]
fn floating_point() {
    equiv_all(&[
        "int main(void){double d=1.5;return (int)(d*4);}",
        "int main(void){float f=0.5f;return (int)(f*8);}",
        "int main(void){double a=7.0,b=2.0;return (int)(a/b);}",
        "int main(void){double a=7.0,b=2.0;return (int)(a-b);}",
        "int main(void){double d=-2.5;return (int)(-d);}",
        "int main(void){double a=1.0,b=2.0;return (a<b)+(a==b)+(a!=b)+(a>=b);}",
        "int main(void){int i=-7;double d=i;return (int)d;}",
        "int main(void){unsigned u=4000000000u;double d=u;return (int)(d/1000000);}",
        "int main(void){char c=-3;double d=c;return (int)d;}",
        "int main(void){float f=1.25f;double d=f;return (int)(d*4);}",
        "int main(void){double d=1.25;float f=d;return (int)(f*4);}",
        "double h(double x){return x/2;} int main(void){return (int)h(9.0);}",
    ]);
}

#[test]
fn memory_and_addresses() {
    equiv_all(&[
        "int main(void){int a[5],i;for(i=0;i<5;i++)a[i]=i*i;return a[4];}",
        "int main(void){int x=7,*p=&x;*p=9;return x;}",
        "int g=10; int main(void){g+=5;return g;}",
        "int a[3]={1,2,3}; int main(void){return a[0]+a[1]+a[2];}",
        "int main(void){char s[]=\"abc\";return s[0]+s[2];}",
        "const char *s=\"hi\"; int main(void){return s[1];}",
        "struct P{int x,y;}; int main(void){struct P p;p.x=3;p.y=4;return p.x*p.y;}",
        "struct P{int x;double d;}; int main(void){struct P p;p.x=3;p.d=1.5;return p.x+(int)p.d;}",
    ]);
}

#[test]
fn calls_and_returns() {
    equiv_all(&[
        "int f(int x){return x*2;} int main(void){return f(21);}",
        "int fib(int n){return n<2?n:fib(n-1)+fib(n-2);} int main(void){return fib(12);}",
        "int a8(int a,int b,int c,int d,int e,int f,int g,int h){return a+b+c+d+e+f+g+h;}\
         int main(void){return a8(1,2,3,4,5,6,7,8);}",
        "double d2(double a,double b){return a+b;} int main(void){return (int)d2(1.5,2.5);}",
        "int g(void){return 3;} int main(void){int (*p)(void)=g;return p()*14;}",
        "void v(int *p){*p=9;} int main(void){int x=0;v(&x);return x;}",
    ]);
}

#[test]
fn switch_lowers_to_a_compare_chain() {
    equiv_all(&[
        "int main(void){int x=2,r=0;switch(x){case 1:r=10;break;case 2:r=20;case 3:r+=3;break;default:r=99;}return r;}",
        "int main(void){int x=9,r=0;switch(x){case 1:r=1;break;default:r=7;}return r;}",
        "int main(void){int s=0,i;for(i=0;i<6;i++){switch(i){case 0:case 2:case 4:s+=i;break;default:s-=1;}}return s;}",
        "int main(void){int x=100000,r=0;switch(x){case 100000:r=5;break;default:r=1;}return r;}",
    ]);
}

#[test]
fn block_arguments_survive_selection() {
    // `?:` and short-circuit operators are the only source of block parameters
    // before R2; they are what SSA destruction will later have to sequentialize.
    equiv_all(&[
        "int main(void){int a=1,b=2;return (a<b?a:b)+(a>b?a:b);}",
        "int main(void){int a=0,b=3;return (a&&b)+(a||b);}",
        "int main(void){int i=3;return i>0 ? (i>2 ? 7 : 8) : 9;}",
        "double m(double a,double b){return a<b?a:b;} int main(void){return (int)m(2.0,5.0);}",
    ]);
}


// ── R1 selection rules (REARCH §15) ────────────────────────────────────────
// These were vacuous until `hir::interp` learned the intrinsics: a variadic or
// long-double function trapped on the HIR side, so `equiv` compared nothing.

#[test]
fn r1_lowering_rules() {
    equiv_all(&[
        // bit-fields: the shift pair and the read-modify-write
        "struct B{int a:3;unsigned b:5;int c:10;};\n\
         int main(void){struct B s;s.a=-2;s.b=19;s.c=-300;return s.a*10000+s.b*100+s.c;}",
        "struct B{int a:3;};int main(void){struct B s;s.a=3;s.a++;return s.a;}",
        // composites: registers, x8, HFA, and the stack
        "struct P{int x,y;};struct P mk(int a){struct P p;p.x=a;p.y=a*2;return p;}\n\
         int main(void){struct P q=mk(5);return q.x*100+q.y;}",
        "struct B{long a,b,c,d;};struct B mk(long x){struct B r;r.a=x;r.b=x+1;r.c=x+2;r.d=x+3;return r;}\n\
         long sum(struct B s){return s.a+s.b+s.c+s.d;}int main(void){return (int)sum(mk(10));}",
        "struct H{float a,b,c,d;};int s(struct H h){return (int)(h.a+h.b+h.c+h.d);}\n\
         int main(void){struct H h;h.a=1;h.b=2;h.c=3;h.d=4;return s(h);}",
        "int f(int a,int b,int c,int d,int e,int g,int h,int i,int j){return j*10+i;}\n\
         int main(void){return f(1,2,3,4,5,6,7,8,9);}",
        // varargs, in both files and over the register boundary
        "int s(int n,...){__builtin_va_list a;int i,t=0;__builtin_va_start(a,n);\
         for(i=0;i<n;i++)t+=__builtin_va_arg(a,int);return t;}\n\
         int main(void){return s(12,1,2,3,4,5,6,7,8,9,10,11,12);}",
        "double f(int n,...){__builtin_va_list a;double s=0;int i;__builtin_va_start(a,n);\
         for(i=0;i<n;i++)s+=__builtin_va_arg(a,double);return s;}\n\
         int main(void){return (int)f(10,1.,2.,3.,4.,5.,6.,7.,8.,9.,10.);}",
        // long double: the binary128 bridge at every boundary
        "long double id(long double x){return x;}int main(void){return (int)(id(3.5L)*2);}",
        "int main(void){long double x=2.5L;x+=1.0L;return (int)(x*2);}",
        "long double many(long double a,long double b,long double c,long double d,\
         long double e,long double f,long double g,long double h,long double i,long double j)\
         {return a+b+c+d+e+f+g+h+i+j;}\n\
         int main(void){return (int)many(1.L,2,3,4,5,6,7,8,9,10);}",
        // atomics and the overflow builtins
        "int main(void){int i=10;int o=__sync_fetch_and_add(&i,5);return o*100+i;}",
        "int main(void){int i=17;return __sync_bool_compare_and_swap(&i,17,99)*100+i;}",
        "int main(void){int r;return __builtin_mul_overflow(100000,100000,&r)*10+r;}",
        // switch ranges and the promoted controlling expression
        "int f(int x){switch(x){case 10 ... 20: return 1;case 30: return 2;}return 0;}\n\
         int main(void){return f(15)*100+f(30)*10+f(25);}",
        "int g(signed char c){switch(c){case -62: return 19;}return 0;}int main(void){return g(-62);}",
        // VLA and a dynamic frame
        "int sum(int n){int a[n];int i,s=0;for(i=0;i<n;i++)a[i]=i*i;\
         for(i=0;i<n;i++)s+=a[i];return s;}int main(void){return sum(10);}",
    ]);
}

/// AAPCS64 §6.4 C.1–C.15, checked against the SPEC's own answers rather than
/// against whatever `classify` currently returns.
#[test]
fn abi_classification_matches_the_spec() {
    use super::abi::{Loc, classify};
    use crate::hir::{PTy, Sig, Ty};
    use crate::mir::{PReg, Width};
    let sig = |params: Vec<PTy>, ret: Option<PTy>| Sig {
        nfix: params.len() as u32,
        params,
        ret,
        variadic: false,
    };
    let agg = |size, align, hfa| PTy::Agg { size, align, hfa };

    // C.9: integers take x0.. in order; C.1: floats take v0.. independently
    let a = classify(&sig(
        vec![PTy::S(Ty::I32), PTy::S(Ty::F64), PTy::S(Ty::I64), PTy::S(Ty::F32)],
        None,
    ));
    assert_eq!(a.args[0], Loc::Reg(PReg::gpr(0), Width::W32));
    assert_eq!(a.args[1], Loc::Reg(PReg::fpr(0), Width::D));
    assert_eq!(a.args[2], Loc::Reg(PReg::gpr(1), Width::W64));
    assert_eq!(a.args[3], Loc::Reg(PReg::fpr(1), Width::S));

    // C.14 + C.16: past x7 the NSAA is rounded to 8 AND the argument occupies a
    // full 8 bytes however narrow it is (measured against gcc: char, short, int
    // land at [sp,0], [sp,8], [sp,16])
    let mut p = vec![PTy::S(Ty::I64); 8];
    p.extend([PTy::S(Ty::I8), PTy::S(Ty::I16), PTy::S(Ty::I32)]);
    let a = classify(&sig(p, None));
    assert_eq!(a.args[8], Loc::Stack(0, Width::W32));
    assert_eq!(a.args[9], Loc::Stack(8, Width::W32));
    assert_eq!(a.args[10], Loc::Stack(16, Width::W32));

    // §6.8.2: a composite of 16 bytes or fewer takes ⌈size/8⌉ x-registers…
    let a = classify(&sig(vec![agg(16, 8, None), PTy::S(Ty::I32)], None));
    assert_eq!(a.args[0], Loc::Regs { first: PReg::gpr(0), n: 2, esz: 8, size: 16 });
    assert_eq!(a.args[1], Loc::Reg(PReg::gpr(2), Width::W32));
    // …and one larger is replaced by a POINTER, i.e. one ordinary x-register
    assert_eq!(
        classify(&sig(vec![agg(32, 8, None)], None)).args[0],
        Loc::Reg(PReg::gpr(0), Width::W64)
    );
    // §5.9.5: an HFA takes consecutive v-registers, one per element
    assert_eq!(
        classify(&sig(vec![agg(16, 4, Some((false, 4)))], None)).args[0],
        Loc::Regs { first: PReg::fpr(0), n: 4, esz: 4, size: 16 }
    );
    // C.3: an HFA that does not fit LOCKS the remaining v-registers, so a later
    // float goes to the stack rather than into the gap
    let a = classify(&sig(
        vec![PTy::S(Ty::F64); 6]
            .into_iter()
            .chain([agg(32, 8, Some((true, 4))), PTy::S(Ty::F64)])
            .collect(),
        None,
    ));
    assert_eq!(a.args[6], Loc::StackAgg { off: 0, size: 32 });
    assert_eq!(a.args[7], Loc::Stack(32, Width::D));
    // C.11: a composite that does not fit locks NGRN the same way
    let a = classify(&sig(
        vec![PTy::S(Ty::I64); 7]
            .into_iter()
            .chain([agg(16, 8, None), PTy::S(Ty::I64)])
            .collect(),
        None,
    ));
    assert_eq!(a.args[7], Loc::StackAgg { off: 0, size: 16 });
    assert_eq!(a.args[8], Loc::Stack(16, Width::W64));
    // over-alignment is IGNORED (gcc places an aligned(32) composite at [sp,0])
    let a = classify(&sig(
        vec![PTy::S(Ty::I64); 8]
            .into_iter()
            .chain([agg(32, 32, None)])
            .collect(),
        None,
    ));
    assert_eq!(a.args[8], Loc::Stack(0, Width::W64));

    // §5.1.2: binary128 is a whole v-register, or 16 stack bytes aligned to 16
    assert_eq!(
        classify(&sig(vec![PTy::LDouble], None)).args[0],
        Loc::Reg(PReg::fpr(0), Width::Q)
    );

    // §6.9: HFA in v0.., ≤16 bytes in x0..x1, anything larger through x8
    assert!(!classify(&sig(vec![], Some(agg(32, 8, Some((true, 4)))))).sret);
    assert_eq!(
        classify(&sig(vec![], Some(agg(16, 8, None)))).ret,
        Some(Loc::Regs { first: PReg::gpr(0), n: 2, esz: 8, size: 16 })
    );
    let a = classify(&sig(vec![], Some(agg(24, 8, None))));
    assert!(a.sret && a.ret.is_none());

    // the counters `va_start` publishes, after the NAMED parameters only
    let a = classify(&sig(vec![PTy::S(Ty::I32), PTy::S(Ty::F64), PTy::S(Ty::I32)], None));
    assert_eq!((a.ngrn, a.nsrn, a.nsaa), (2, 1, 0));
}

#[test]
fn a_null_address_is_materialized_not_ridden_in_zr() {
    // DDI 0487 C1.2.5: Rn = 31 in a load/store decodes as SP, not ZR. A data
    // operand may ride in the zero register for free; a BASE may not. The HIR
    // ladder is what makes this reachable — `*(char *)0 = 0` only becomes a
    // literal address after the cast folds (torture 930719-1). `mir::verify`
    // now refuses it, so this battery fails at the MIR layer rather than in the
    // assembler.
    equiv("int main(void){char *p = (char *)0; if (0) *p = 0; return 0;}");
    equiv("int f(int c){if(c) *(char *)0 = 0; return c;}int main(void){return f(0);}");
    equiv("int f(int c){if(c) return *(char *)0; return 7;}int main(void){return f(0);}");
}

#[test]
fn an_address_rides_in_the_memory_operand() {
    // The R1 ground metric measured `add` at 28.2% of sqlite's instructions,
    // every one of them computing an address the load could have held itself.
    // These are the shapes that must fold: a frame object at a constant
    // displacement, and a pointer plus a constant.
    equiv("int main(void){int a[4];a[0]=1;a[1]=2;a[2]=3;a[3]=4;return a[0]+a[1]+a[2]+a[3];}");
    equiv("struct P{int x,y,z;};int f(struct P*p){return p->x+p->y+p->z;}\
           int main(void){struct P p;p.x=1;p.y=2;p.z=3;return f(&p);}");
    // …and the shape that must NOT: a displacement past the encodable range
    // (DDI 0487 C3.2 scales the unsigned form by the access size).
    equiv("int f(int*p){return p[100000];}\
           int main(void){int a[1];a[0]=7;return f(a-100000);}");
}

#[test]
fn a_compare_feeding_only_a_branch_needs_no_cset() {
    // `cmp` + `b.cc`, not `cmp` + `cset` + `cbnz`. The fusion is legal only when
    // the compare has ONE use — otherwise the 0/1 value is still needed.
    equiv("int f(int a,int b){if(a<b)return 1;return 2;}int main(void){return f(1,2);}");
    equiv("int f(unsigned a,unsigned b){if(a>=b)return 1;return 2;}int main(void){return f(1,2);}");
    equiv("double f(double a,double b){if(a<b)return 1;return 2;}int main(void){return (int)f(1,2);}");
    // two uses: the value is materialized AND branched on
    equiv("int f(int a,int b){int c=a<b;if(c)return c+1;return c+2;}int main(void){return f(1,2);}");
    // NaN: an unordered compare must keep its own condition code
    equiv("int f(double a,double b){if(a!=b)return 1;return 2;}int main(void){return f(0.0,0.0);}");
}

#[test]
fn a_dense_switch_becomes_a_jump_table() {
    // A compare chain costs one test per case on the way to the right arm; a
    // table costs four instructions whatever the case. The range check is part
    // of the theorem, not an optimization: `sub` then an UNSIGNED compare
    // rejects everything outside [lo, hi] in one branch, because a value below
    // `lo` wraps to a huge unsigned number.
    equiv(
        "int f(int x){switch(x){case 0:return 10;case 1:return 11;case 2:return 12;\
         case 3:return 13;case 4:return 14;case 5:return 15;default:return 99;}}\
         int main(void){int s=0,i;for(i=-2;i<9;i++)s+=f(i);return s;}",
    );
    // a NEGATIVE base: the subtraction is what makes the table start at zero
    equiv(
        "int f(int x){switch(x){case -3:return 1;case -2:return 2;case -1:return 3;\
         case 0:return 4;case 1:return 5;default:return 0;}}\
         int main(void){int s=0,i;for(i=-5;i<4;i++)s+=f(i);return s;}",
    );
    // SPARSE: a table would be mostly padding, so the chain must survive
    equiv(
        "int f(int x){switch(x){case 1:return 1;case 1000:return 2;case 100000:return 3;\
         default:return 4;}}\
         int main(void){return f(1)+f(1000)+f(100000)+f(7);}",
    );
    // too FEW cases for a table
    equiv("int f(int x){switch(x){case 1:return 5;case 2:return 6;default:return 7;}}\
           int main(void){return f(1)+f(2)+f(3);}");
}

#[test]
fn adjacent_accesses_become_a_pair() {
    // `ldp`/`stp`: the shape the prologue, the epilogue and the spiller all
    // produce. The battery's obligation is the square; the counts are on the
    // corpus.
    equiv("struct P{long a,b;};long f(struct P*p){return p->a+p->b;}\
           int main(void){struct P p;p.a=40;p.b=2;return (int)f(&p);}");
    equiv("void g(long*p){p[0]=1;p[1]=2;}\
           int main(void){long a[2];g(a);return (int)(a[0]*10+a[1]);}");
    // a byte access has no paired form
    equiv("void g(char*p){p[0]=1;p[1]=2;}\
           int main(void){char a[2];g(a);return a[0]*10+a[1];}");
}

#[test]
fn a_compare_feeding_only_a_select_needs_no_cset() {
    // `cmp` + `csel`, not `cmp` + `cset` + `cmp` + `csel`. The compare is
    // RE-EMITTED at its consumer rather than moved, so its flags live for one
    // instruction — NZCV is a register class of size one, and a compare that
    // travelled would collide with the next one.
    equiv("int f(int a,int b){return a<b?a:b;}int main(void){return f(42,50);}");
    equiv("unsigned f(unsigned a,unsigned b){return a>b?a-b:b-a;}\
           int main(void){return (int)f(50,8);}");
    equiv("int f(int a,int b,int c){return (a==b)?c:(a<b?c+1:c+2);}\
           int main(void){return f(1,2,41);}");
}

#[test]
fn a_bitfield_read_is_one_instruction() {
    // C spells a bitfield read as a shift and a mask, or as a pair of shifts;
    // A64 has `ubfx`/`sbfx` for each (DDI 0487 C6.2.398). The chained case is
    // the one that broke first: a fold that has itself absorbed something may
    // not be absorbed again, or the value it swallowed is defined nowhere.
    equiv("struct B{int a:3;unsigned b:5;int c:10;};\
           int main(void){struct B s;s.a=-2;s.b=19;s.c=-300;return s.a*10000+s.b*100+s.c;}");
    equiv("unsigned f(unsigned x){return (x>>7)&0x1f;}\
           int main(void){return (int)f(0xabcdu);}");
    equiv("int f(int x){return (x<<20)>>27;}int main(void){return f(-1)+f(1024);}");
    equiv("unsigned f(unsigned x){return (x<<20)>>27;}int main(void){return (int)f(0xffffffffu);}");
    // an offset past the register: not a bitfield, and must not become one
    equiv("unsigned f(unsigned x){return (x>>30)&0xff;}int main(void){return (int)f(0xc0000000u);}");
}

// ── R4.7: the §17 rows, each with its square and its count ────────────────
// A row here has TWO obligations. The square (`equiv`) says the machine
// sequence means what the C means; the COUNT says the row actually fired —
// §13n's finding (f) was precisely that §17's ✔ marks were claims and the
// mnemonics were measurably absent. A square alone would have stayed green
// with nothing selected.

/// The MIR of one function, before register allocation.
fn mir_of(src: &str, name: &str) -> crate::mir::MFunc {
    let ast = frontend(src);
    let mut h = hir::build::build(&ast);
    hir::pass::run_module(&mut h);
    let m = lower(&h);
    m.funcs
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function `{}`", name))
}

fn count(f: &crate::mir::MFunc, p: impl Fn(&crate::mir::MInst) -> bool) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| p(i)).count()
}

#[test]
fn a_sign_extending_load_needs_no_extension() {
    use crate::mir::{MInst, MemOp};
    // `ldrsh w0,[x0]`, not `ldrh w0,[x0]` + `sxth w0,w0` (DDI 0487 C6.2.192).
    let f = mir_of("int f(short*p){return *p;} int main(void){short x=-5;return f(&x);}", "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::Load { op: MemOp::SH, .. })), 1);
    assert_eq!(count(&f, |i| matches!(i, MInst::Ext { .. })), 0);
    equiv("int f(short*p){return *p;} int main(void){short x=-5;return f(&x);}");
    equiv("int f(signed char*p){return *p;} int main(void){signed char x=-5;return f(&x);}");
    equiv("long f(int*p){return *p;} int main(void){int x=-5;return (int)f(&x);}");
    // …and the loop-carried case the row exists for: the extension in the LOAD
    // leaves the accumulate a plain 1-cycle `add` instead of `add …,sxtw`.
    equiv("long g(int*a,int n){long s=0;int i;for(i=0;i<n;i++)s+=a[i];return s;}\
           int main(void){int a[4];int i;for(i=0;i<4;i++)a[i]=i-2;return (int)g(a,4);}");
}

#[test]
fn the_extension_width_belongs_to_the_opcode_not_the_register() {
    use crate::mir::{MInst, MemOp};
    // `ldrsb Wt` and `ldrsb Xt` are DIFFERENT instructions: the `w` form zeroes
    // bits 63:32. After allocation the destination is physical and carries no
    // width, so the form must be in the opcode — inferring it from the register
    // printed `ldrsb x0` for a 32-bit extension and computed
    // `(unsigned)(signed char)-4` as −4 (torture pr19606).
    let src = "signed char a=-4;\
               int f(void){return ((unsigned)(int)a)/2LL;}\
               int main(void){return f()==2147483646;}";
    let f = mir_of(src, "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::Load { op: MemOp::SB, .. })), 1);
    assert_eq!(count(&f, |i| matches!(i, MInst::Load { op: MemOp::SBX, .. })), 0);
    equiv(src);
    equiv("signed char a=-4; long f(void){return a;} int main(void){return (int)f()+4;}");
}

#[test]
fn a_single_bit_test_is_one_branch() {
    use crate::mir::{AluOp, MInst, MTerm};
    // `tbz`/`tbnz` (DDI 0487 C6.2.375): no mask, no compare.
    // A store in the arm keeps this a BRANCH — `if-conv` would otherwise turn
    // the value-only shape into a `csel`, which is a different (and correct)
    // answer that does not exercise this row.
    let src = "int f(int x,int*p){if(x&8){*p=1;return 1;}return 0;}\
               int main(void){int q=0;return f(8,&q)+f(4,&q)+q;}";
    let f = mir_of(src, "f");
    assert!(f.blocks.iter().any(|b| matches!(b.term, MTerm::Tb { .. })));
    assert_eq!(count(&f, |i| matches!(i, MInst::Alu { op: AluOp::And, .. })), 0);
    equiv(src);
    equiv("int f(unsigned x){return (x&0x80000000u)?7:9;} int main(void){return f(0x80000000u);}");
    // a MULTI-bit mask has no `tb` form and must keep the mask
    equiv("int f(int x,int*p){if(x&12){*p=1;return 1;}return 0;}\
           int main(void){int q=0;return f(8,&q)+f(3,&q)+q;}");
}

#[test]
fn the_conditional_select_family_absorbs_its_arithmetic() {
    use crate::mir::{CSelOp, MInst};
    // `csneg`/`csinv`/`csinc` (DDI 0487 C6.2.83-86) perform the negation,
    // complement or increment on the second source AS PART of the select.
    let src = "int f(int c,int x){return c?x:-x;} int main(void){return f(0,-42);}";
    let f = mir_of(src, "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::CSel { op: CSelOp::Csneg, .. })), 1);
    equiv(src);
    // `c ? 1 : 0` is `cset` alone — the 1 is never materialized.
    let src = "int f(int a,int b,int c){return (a<b&&b<c)?11:22;} int main(void){return f(1,2,3);}";
    let g = mir_of(src, "f");
    assert_eq!(count(&g, |i| matches!(i, MInst::MovImm { imm: 1, .. })), 0);
    equiv(src);
    equiv("int f(int c,int x){return c?-x:x;} int main(void){return f(1,-42);}");
    equiv("int f(int c,int x){return c?x:~x;} int main(void){return f(0,-43);}");
    equiv("int f(int c,int x){return c?x:x+1;} int main(void){return f(0,41);}");
}

#[test]
fn a_constant_operand_reaches_the_immediate_field_from_either_side() {
    use crate::mir::{CmpKind, MInst};
    // A64's immediate field is on the SECOND operand only, so `7 < x` must be
    // read as `x > 7`; and `cmp x,#-1` has no encoding while `cmn x,#1` is the
    // same arithmetic and the same NZCV (DDI 0487 C6.2.62).
    let src = "int f(int x){return 7<x;} int main(void){return f(9);}";
    let f = mir_of(src, "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::MovImm { .. })), 0);
    equiv(src);
    let src = "int f(int x){return x==-1;} int main(void){return f(-1)+f(3);}";
    let g = mir_of(src, "f");
    assert_eq!(count(&g, |i| matches!(i, MInst::Cmp { kind: CmpKind::Cmn, .. })), 1);
    equiv(src);
    equiv("int f(int x){return -3<=x;} int main(void){return f(-4)+f(0);}");
    equiv("int f(unsigned x){return 5u>x;} int main(void){return f(1)+f(9);}");
    equiv("int f(int x){return x>-4096;} int main(void){return f(-5000)+f(0);}");
}

#[test]
fn a_shift_folds_into_a_commutative_operation_from_either_side() {
    use crate::mir::{AluOp, MInst, Rhs};
    // A64 shifts the SECOND source; C writes the shifted side wherever it
    // likes. `orr w0,w1,w2,lsl #1`, not `lsl` + `orr`.
    let src = "unsigned f(unsigned x,unsigned y){return (x<<1)|y;} int main(void){return (int)f(3,4);}";
    let f = mir_of(src, "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::Alu { op: AluOp::Lsl, .. })), 0);
    assert_eq!(
        count(&f, |i| matches!(i, MInst::Alu { op: AluOp::Orr, b: Rhs::Shifted(..), .. })),
        1
    );
    equiv(src);
    equiv("unsigned f(unsigned x,unsigned y){return (x<<3)+y;} int main(void){return (int)f(3,4);}");
    equiv("unsigned f(unsigned x,unsigned y){return (x>>3)^y;} int main(void){return (int)f(64,4);}");
    // subtraction does NOT commute
    equiv("unsigned f(unsigned x,unsigned y){return (x<<2)-y;} int main(void){return (int)f(3,4);}");
    equiv("unsigned f(unsigned x,unsigned y){return y-(x<<2);} int main(void){return (int)f(3,40);}");
}

#[test]
fn multiply_accumulate_takes_a_literal_multiplier() {
    use crate::mir::{Alu3Op, AluOp, MInst};
    // §17 row 23's category-(b) residual. A LITERAL multiplier is not a reason
    // to leave the `add` standing: the literal has to reach a register before
    // `mul` can read it either way, so `madd` costs the same register and one
    // instruction less — and it shortens the loop-carried chain of
    // `a = a*K + C`, which is what the hot loop of `tests/bench/loops.c` is.
    let src = "unsigned long f(unsigned long a){return a*1103515245UL+12345UL;}\n\
               int main(void){return (int)f(1);}";
    let f = mir_of(src, "f");
    assert_eq!(count(&f, |i| matches!(i, MInst::Alu3 { op: Alu3Op::Madd, .. })), 1);
    assert_eq!(count(&f, |i| matches!(i, MInst::Alu { op: AluOp::Mul, .. })), 0);
    equiv(src);
    // `c − a*K` is the same row through `msub`
    let g = "unsigned long f(unsigned long a,unsigned long c){return c-a*3141592653UL;}\n\
             int main(void){return (int)f(7,9);}";
    assert_eq!(count(&mir_of(g, "f"), |i| matches!(i, MInst::Alu3 { op: Alu3Op::Msub, .. })), 1);
    equiv(g);
    // the value-operand form the row already had, unchanged
    equiv("long f(long a,long b,long c){return a*b+c;} int main(void){return (int)f(3,4,5);}");
    equiv("int f(int a,int c){return a*7+c;} int main(void){return f(3,4);}");
}

/// MEASURED M14 — a small copy is open-coded, and a by-value struct parameter
/// is the case that pays for it.
///
/// C 6.9.1p9: a parameter is a local object, so the frontend homes an aggregate
/// one by copying the incoming registers into the local's storage. That is a
/// sixteen-byte `MemCpy` for a four-`int` struct, and lowering it to
/// `bl memcpy` cost far more than the copy: the call, a frame and an x30 save
/// in a function that is otherwise a LEAF, and the caller-saved half clobbered
/// while the argument registers are still live. `e3_struct_byval` measured
/// 2.630x gcc -O1 — the worst program in the taxonomy suite on both axes — for
/// a copy gcc does not make at all.
///
/// NON-VACUOUS: with `INLINE_COPY_MAX = 0` this function contains exactly one
/// `bl memcpy` and the first assertion fails.
#[test]
fn a_small_struct_copy_is_open_coded_not_called() {
    use crate::mir::{CallTarget, MInst};
    let src = "struct V{ int a,b,c,d; };\n\
               long sum(struct V v){ return (long)v.a + v.b - v.c + v.d; }\n\
               int main(void){ struct V v; v.a=1; v.b=2; v.c=3; v.d=4;\n\
               return (int)sum(v); }";
    let f = mir_of(src, "sum");
    let calls = count(&f, |i| {
        matches!(i, MInst::Call { callee: CallTarget::Direct(n), .. } if n == "memcpy")
    });
    assert_eq!(calls, 0, "a 16-byte parameter home still goes through libc");
    // and it is a copy, not a deletion: the bytes still move
    assert!(
        count(&f, |i| matches!(i, MInst::Load { .. })) >= 2
            && count(&f, |i| matches!(i, MInst::Store { .. })) >= 2,
        "the copy vanished instead of being open-coded"
    );
    equiv(src);
    // past the bound it stays a call — the bound is a real boundary, not a
    // direction of travel
    let big = "struct W{ int v[64]; };\n\
               long tot(struct W w){ long s=0; int i; for(i=0;i<64;i++) s+=w.v[i]; return s; }\n\
               int main(void){ struct W w; int i; for(i=0;i<64;i++) w.v[i]=i;\n\
               return (int)tot(w); }";
    assert!(
        count(&mir_of(big, "tot"), |i| {
            matches!(i, MInst::Call { callee: CallTarget::Direct(n), .. } if n == "memcpy")
        }) >= 1,
        "a 256-byte copy was open-coded, which MEASURED M14 says costs size"
    );
    equiv(big);
}
