// The HIR battery (REARCH.md §12 R0.4, §10 row "AST → HIR").
//
// Law 3, at the earliest layer where the question is decidable: each case below
// is a C program whose value is fixed by the STANDARD (an independent oracle,
// transcribed by hand — never by asking zcc what it currently prints), and the
// battery checks that ⟦build(parse(src))⟧ produces exactly that value. A
// mismatch localizes to the lowering of one construct, before any machine layer
// exists to hide it.
//
// Later milestones extend the same harness: an HIR→HIR pass P is proven by
// running BOTH sides here and comparing, which is the commuting square ⟦f⟧=⟦P f⟧.
use super::interp::{Trap, new_machine};
use super::{build, verify};
use crate::testutil::frontend;

/// Frontend + lowering + verifier, on a source string.
fn hir_of(src: &str) -> super::Module {
    let ast = frontend(src);
    let m = build::build(&ast);
    for f in &m.funcs {
        verify::verify(f).expect("hir verifier");
    }
    m
}

/// Run `main()` under ⟦hir⟧ and return its value.
fn run(src: &str) -> Result<i64, Trap> {
    let ast = frontend(src);
    let m = build::build(&ast);
    for f in &m.funcs {
        verify::verify(f).unwrap_or_else(|e| panic!("{}", e));
    }
    let mut mach = new_machine(&m, &ast);
    mach.call("main", &[]).map(|r| r.unwrap_or(0) as i32 as i64)
}

/// `src` must evaluate to `want` — the value C99 assigns it.
fn check(src: &str, want: i64) {
    match run(src) {
        Ok(got) if got == want => {}
        Ok(got) => panic!("⟦hir⟧ = {} but C99 says {}\n{}", got, want, src),
        Err(t) => panic!("⟦hir⟧ trapped: {:?}\n{}", t, src),
    }
}

#[test]
fn constants_and_arithmetic() {
    check("int main(void){return 42;}", 42);
    check("int main(void){return 6*7;}", 42);
    check("int main(void){return 100/7;}", 14);
    check("int main(void){return 100%7;}", 2);
    check("int main(void){return -5/2;}", -2); // C99 6.5.5p6: truncation toward zero
    check("int main(void){return -5%2;}", -1);
    check("int main(void){return 1+2*3-4/2;}", 5);
    check("int main(void){return (1<<10)-1;}", 1023);
    check("int main(void){return -1>>1;}", -1); // arithmetic on a signed value
    check("int main(void){unsigned x=-1;return x>>28;}", 15);
    check("int main(void){return 0xf0 & 0x3c;}", 0x30);
    check("int main(void){return 0xf0 | 0x0f;}", 0xff);
    check("int main(void){return 0xff ^ 0x0f;}", 0xf0);
    check("int main(void){return ~0;}", -1);
}

#[test]
fn integer_width_and_signedness() {
    // C99 6.3.1.3: conversion to a signed type is modular here (2's complement)
    check("int main(void){int x=300;char c=x;return c;}", 44);
    check("int main(void){int x=300;unsigned char c=x;return c;}", 44);
    check("int main(void){short s=-1;return (int)s;}", -1);
    check("int main(void){short s=-1;return (unsigned short)s;}", 65535);
    check("int main(void){long x=1;return (int)(x<<40>>40);}", 1);
    check("int main(void){unsigned a=1,b=2;return a-b>0;}", 1); // unsigned wrap
    check("int main(void){int a=1,b=2;return a-b>0;}", 0);
}

#[test]
fn comparisons_and_logic() {
    check("int main(void){return 1<2;}", 1);
    check("int main(void){return 2<=2;}", 1);
    check("int main(void){return 3>4;}", 0);
    check("int main(void){return 1==1 && 2!=3;}", 1);
    check("int main(void){return 0 && 1/0;}", 0); // 6.5.13p4: && does not evaluate rhs
    check("int main(void){return 1 || 1/0;}", 1);
    check("int main(void){return !0 + !1;}", 1);
    check("int main(void){int x=5;return x>3 ? 10 : 20;}", 10);
    check("int main(void){int x=1;return x?x?2:3:4;}", 2);
}

#[test]
fn control_flow() {
    check("int main(void){int s=0,i;for(i=0;i<10;i++)s+=i;return s;}", 45);
    check("int main(void){int i=0,s=0;while(i<5){s+=i;i++;}return s;}", 10);
    check("int main(void){int i=0,s=0;do{s+=i;i++;}while(i<5);return s;}", 10);
    check(
        "int main(void){int i,s=0;for(i=0;i<10;i++){if(i==3)continue;if(i==6)break;s+=i;}return s;}",
        1 + 2 + 4 + 5,
    );
    check("int main(void){int i=0;again:i++;if(i<7)goto again;return i;}", 7);
    check(
        "int main(void){int x=2,r=0;switch(x){case 1:r=10;break;case 2:r=20;case 3:r+=3;break;default:r=99;}return r;}",
        23,
    );
    check(
        "int main(void){int x=9,r=0;switch(x){case 1:r=1;break;default:r=7;}return r;}",
        7,
    );
}

#[test]
fn functions_and_recursion() {
    check("int f(int x){return x*2;} int main(void){return f(21);}", 42);
    check(
        "int fib(int n){return n<2?n:fib(n-1)+fib(n-2);} int main(void){return fib(15);}",
        610,
    );
    check(
        "int add(int a,int b,int c,int d,int e,int f,int g,int h,int i){return a+b+c+d+e+f+g+h+i;}\
         int main(void){return add(1,2,3,4,5,6,7,8,9);}",
        45,
    );
    check("int g(void){return 3;} int main(void){int (*p)(void)=g;return p()*14;}", 42);
}

#[test]
fn pointers_arrays_globals() {
    check("int main(void){int a[5],i;for(i=0;i<5;i++)a[i]=i*i;return a[4];}", 16);
    check("int main(void){int x=7,*p=&x;*p=9;return x;}", 9);
    check("int a[3]={1,2,3}; int main(void){return a[0]+a[1]+a[2];}", 6);
    check("int g=10; int main(void){g+=5;return g;}", 15);
    check(
        "int main(void){int a[4]={1,2,3,4};int *p=a;return *(p+2)+p[3];}",
        7,
    );
    check("int main(void){char s[]=\"abc\";return s[0]+s[2];}", 97 + 99);
    check("int main(void){const char *s=\"hi\";return s[1];}", 105);
    check(
        "struct P{int x,y;}; int main(void){struct P p;p.x=3;p.y=4;return p.x*p.y;}",
        12,
    );
    check(
        "struct P{int x,y;}; int main(void){struct P p,*q=&p;q->x=5;q->y=6;return p.x+p.y;}",
        11,
    );
}

#[test]
fn floating_point() {
    check("int main(void){double d=1.5;return (int)(d*4);}", 6);
    check("int main(void){float f=0.5f;return (int)(f*8);}", 4);
    check("int main(void){double d=3.0;return d>2.5;}", 1);
    check("int main(void){double d=-2.7;return (int)d;}", -2); // 6.3.1.4: toward zero
    check("int main(void){int i=7;double d=i;return (int)(d/2);}", 3);
    check("double h(double x){return x/2;} int main(void){return (int)h(9.0);}", 4);
}

#[test]
fn assignment_forms() {
    check("int main(void){int x=1;x+=2;x*=3;x-=1;x/=2;return x;}", 4);
    check("int main(void){int x=8;x>>=2;x|=1;x&=6;x^=3;return x;}", 1);
    check("int main(void){int x=1,y;y=x++;return y*10+x;}", 12);
    check("int main(void){int x=1,y;y=++x;return y*10+x;}", 22);
    check("int main(void){int a[3]={0,0,0},i=0;a[i++]=5;return a[0]*10+i;}", 51);
    check("int main(void){int x=1,y=2,z;z=(x=3,y=x+1);return z;}", 4);
}

#[test]
fn traps_are_bottom() {
    // ⊥ is not a wrong answer: it is the absence of one. A pass may refine it.
    assert_eq!(run("int main(void){int z=0;return 1/z;}"), Err(Trap::DivZero));
}

#[test]
fn verifier_accepts_every_shape() {
    // The verifier ran inside `hir_of`; reaching here means the SSA dominance
    // property, block-argument arity and opcode typing all hold on these shapes.
    let m = hir_of(
        "int f(int n){int s=0,i;for(i=0;i<n;i++){if(i&1)s+=i;else s-=i;}return s;}\
         int g(int a,int b){return a>b?a:b;}\
         int main(void){return f(10)+g(1,2);}",
    );
    assert_eq!(m.funcs.len(), 3);
}
