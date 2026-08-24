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

/// Run `main` through both layers and require agreement.
fn equiv(src: &str) {
    let ast = frontend(src);
    let h = hir::build::build(&ast);
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
