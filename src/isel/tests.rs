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
