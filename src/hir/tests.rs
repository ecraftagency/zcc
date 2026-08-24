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

// ── R1 lowering batteries (REARCH §15) ─────────────────────────────────────
// R1 shipped its features validated by differential testing alone. These are
// the squares that were owed: each states the value C99 assigns and checks
// ⟦hir⟧ produces it, so csmith/yarpgen go back to CONFIRMING (Law 3).

/// C99 6.7.2.1: a bit-field of width `w` holds exactly the low `w` bits of the
/// value stored, read back under the container's signedness — and a write must
/// leave the neighbouring fields of the same unit untouched.
#[test]
fn bitfields_are_exact_over_their_domain() {
    for w in 1..=12i64 {
        for pre in 1..=3i64 {
            for v in [-100i64, -3, -1, 0, 1, 5, 100, 511] {
                let m = 1i64 << w;
                let low = ((v % m) + m) % m;
                let signed = if low >= m / 2 { low - m } else { low };
                // signed field: value truncates to w bits, sign-extended back
                check(
                    &format!(
                        "struct B{{int p:{pre};int a:{w};}};\n\
                         int main(void){{struct B s;s.p=1;s.a={v};return s.a;}}"
                    ),
                    signed,
                );
                // unsigned field: the same bits, read without a sign
                check(
                    &format!(
                        "struct B{{unsigned p:{pre};unsigned a:{w};}};\n\
                         int main(void){{struct B s;s.p=1;s.a={v};return (int)s.a;}}"
                    ),
                    low,
                );
                // the neighbour survives the read-modify-write
                check(
                    &format!(
                        "struct B{{int p:{pre};int a:{w};}};\n\
                         int main(void){{struct B s;s.p=1;s.a={v};return s.p;}}"
                    ),
                    if pre == 1 { -1 } else { 1 },
                );
            }
        }
    }
}

#[test]
fn bitfield_increment_wraps_inside_the_field() {
    // C99 6.5.2.4 + 6.7.2.1: `a` is 3 bits signed, so 3+1 is 4 → −4.
    check("struct B{int a:3;};int main(void){struct B s;s.a=3;s.a++;return s.a;}", -4);
    check("struct B{int a:3;};int main(void){struct B s;s.a=3;return s.a++;}", 3);
    // compound assignment truncates the same way: 19 + 20 = 39, 39 & 31 = 7
    check(
        "struct B{unsigned b:5;};int main(void){struct B s;s.b=19;s.b+=20;return (int)s.b;}",
        7,
    );
    // the value of the assignment expression is the value STORED (6.5.16p3)
    check("struct B{int a:3;};int main(void){struct B s;return (s.a=5);}", -3);
    // a container narrower than int, and one wider
    check(
        "struct B{unsigned char x:3;unsigned char y:5;};\n\
         int main(void){struct B s;s.x=5;s.y=30;return s.x*100+s.y;}",
        530,
    );
    check(
        "struct B{long q:40;};int main(void){struct B s;s.q=-123456789012L;return (int)(s.q%1000);}",
        -12,
    );
}

/// THEORY II-2: long double is binary128 in memory, computed at double. The
/// round trip through the soft-float pair must be the identity on any value a
/// double can hold.
#[test]
fn long_double_round_trips_through_binary128() {
    check("int main(void){long double x=2.5L;x+=1.0L;return (int)(x*2);}", 7);
    check("int main(void){return (int)sizeof(long double);}", 16);
    check("int main(void){long double a=10;int k=3;return (int)((a/k)*3+0.5);}", 10);
    check("int main(void){long double m=-0.5L;return (m<0.0L)*10+(m==m);}", 11);
    check("long double id(long double x){return x;}int main(void){return (int)(id(3.5L)*2);}", 7);
    // a struct member, so the 16-byte object is loaded and stored through memory
    check(
        "struct S{long double x;char c;};\n\
         int main(void){struct S s;s.x=1.25L;s.c=7;return (int)(s.x*4)+s.c;}",
        12,
    );
    check("struct S{long double x;char c;};int main(void){return (int)sizeof(struct S);}", 32);
}

/// EXT(gcc) `__sync_*` (ARM DDI 0487 B2.9). One thread, so the exclusive pair
/// is an ordinary read-modify-write — which is exactly what the C-level
/// contract of these builtins says the result must be.
#[test]
fn sync_builtins() {
    check("int main(void){int i=10;int o=__sync_fetch_and_add(&i,5);return o*100+i;}", 1015);
    check("int main(void){int i=10;int n=__sync_add_and_fetch(&i,5);return n*100+i;}", 1515);
    check("int main(void){int i=20;int o=__sync_fetch_and_sub(&i,3);return o*100+i;}", 2017);
    check("int main(void){int i=20;int n=__sync_sub_and_fetch(&i,3);return n*100+i;}", 1717);
    check("int main(void){unsigned u=100;unsigned o=__sync_fetch_and_or(&u,15);return o*1000+u;}", 100111);
    check("int main(void){unsigned u=111;unsigned o=__sync_fetch_and_xor(&u,255);return (int)u;}", 144);
    check("int main(void){unsigned u=144;__sync_fetch_and_and(&u,60);return (int)u;}", 16);
    // compare-and-swap: the SECOND argument is the expected value
    check("int main(void){int i=17;int o=__sync_val_compare_and_swap(&i,17,99);return o*1000+i;}", 17099);
    check("int main(void){int i=99;int b=__sync_bool_compare_and_swap(&i,99,7);return b*100+i;}", 107);
    check("int main(void){int i=7;int b=__sync_bool_compare_and_swap(&i,99,8);return b*100+i;}", 7);
    check("int main(void){long l=1000;long o=__sync_lock_test_and_set(&l,555);return (int)(o+l);}", 1555);
    check("int main(void){long l=555;__sync_lock_release(&l);__sync_synchronize();return (int)l;}", 0);
}

/// EXT(gcc) `__builtin_*_overflow`: ℤ semantics — compute at infinite
/// precision, store the truncation, report whether the exact value was
/// representable in the DESTINATION type.
#[test]
fn overflow_builtins_use_infinite_precision() {
    // in range: no overflow, exact result stored
    check("int main(void){int r;int f=__builtin_add_overflow(2,3,&r);return f*100+r;}", 5);
    check("int main(void){int r;int f=__builtin_sub_overflow(2,5,&r);return f*100+r;}", -3);
    check("int main(void){int r;int f=__builtin_mul_overflow(6,7,&r);return f*100+r;}", 42);
    // 32-bit signed overflow
    check("int main(void){int r;return __builtin_add_overflow(2147483647,1,&r);}", 1);
    check("int main(void){int r;return __builtin_sub_overflow(-2147483647-1,1,&r);}", 1);
    check("int main(void){int r;return __builtin_mul_overflow(100000,100000,&r);}", 1);
    // unsigned destination: a negative exact value is not representable
    check("int main(void){unsigned r;return __builtin_sub_overflow(1,2,&r);}", 1);
    check("int main(void){unsigned r;return __builtin_add_overflow(1,2,&r);}", 0);
    // the OPERAND width decides whether the 64-bit step is already exact:
    // int × int always fits i64, so only the sign can make it unrepresentable
    check("int main(void){unsigned long long r;return __builtin_mul_overflow(-2,3,&r);}", 1);
    check("int main(void){unsigned long long r;return __builtin_mul_overflow(2,3,&r);}", 0);
    // 64-bit operands need the carry rule and the high half of the product
    check("int main(void){long r;return __builtin_mul_overflow(4000000000L,4000000000L,&r);}", 1);
    check("int main(void){unsigned long r;return __builtin_add_overflow(18446744073709551615UL,1UL,&r);}", 1);
    // a narrow destination: representability is the round trip. THEORY II-3:
    // plain `char` is UNSIGNED on AArch64-ELF, so 128 fits it and only the
    // explicitly signed one overflows (confirmed against the referee).
    check("int main(void){signed char r;return __builtin_add_overflow(127,1,&r);}", 1);
    check("int main(void){char r;int f=__builtin_add_overflow(127,1,&r);return f*100+r;}", 128);
    check("int main(void){unsigned char r;return __builtin_add_overflow(255,1,&r);}", 1);
    check("int main(void){signed char r;int f=__builtin_add_overflow(120,5,&r);return f*100+r;}", 125);
}

/// EXT(gcc) `case lo ... hi` and C99 6.8.4.2p5's promotion of the controlling
/// expression. A range means exactly the set of values it spans — the property
/// the enumerated form had, kept when enumeration became impossible.
#[test]
fn switch_ranges_and_promotion() {
    let f = "int f(int x){switch(x){case 10 ... 20: return 1; case 30: return 2; \
             case -5 ... -1: return 3;} return 0;}\n";
    for (x, want) in [(9, 0), (10, 1), (15, 1), (20, 1), (21, 0), (30, 2), (-6, 0), (-5, 3), (-1, 3), (0, 0)] {
        check(&format!("{f}int main(void){{return f({x});}}"), want);
    }
    // a range no machine could enumerate
    check(
        "int f(unsigned long long a){switch(a){case 1000000000000000000ULL ... \
         9999999999999999999ULL: return 19; default: return 20;}}\n\
         int main(void){return f(1000000000000000000ULL)*100+f(1);}",
        1920,
    );
    // the controlling expression is PROMOTED, so a negative label matches
    check(
        "int g(signed char c){switch(c){case -62: return 19; case 98: return 18;} return 0;}\n\
         int main(void){return g(-62)*100+g(98);}",
        1918,
    );
    check(
        "int g(unsigned char c){switch(c){case 200: return 7;} return 0;}\n\
         int main(void){return g(200);}",
        7,
    );
}

/// AAPCS64 §B.6: `va_arg` walks the register save area and then the caller's
/// stack area. The battery drives arguments past the 8-register boundary in
/// both files, which is where the two areas must agree with the caller.
#[test]
fn varargs_walk_both_save_areas() {
    let si = "int s(int n,...){__builtin_va_list a;int i,t=0;__builtin_va_start(a,n);\
              for(i=0;i<n;i++)t+=__builtin_va_arg(a,int);return t;}\n";
    check(&format!("{si}int main(void){{return s(4,1,2,3,4);}}"), 10);
    // 12 integer arguments: 7 in x1–x7, the rest on the stack
    check(
        &format!("{si}int main(void){{return s(12,1,2,3,4,5,6,7,8,9,10,11,12);}}"),
        78,
    );
    let sd = "double f(int n,...){__builtin_va_list a;double s=0;int i;__builtin_va_start(a,n);\
              for(i=0;i<n;i++)s+=__builtin_va_arg(a,double);return s;}\n";
    check(&format!("{sd}int main(void){{return (int)f(3,1.5,2.5,3.0);}}"), 7);
    // 10 doubles: 8 in v0–v7, two on the stack
    check(
        &format!("{sd}int main(void){{return (int)f(10,1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0,10.0);}}"),
        55,
    );
    // the two files advance INDEPENDENTLY
    check(
        "int m(int n,...){__builtin_va_list a;int i,t=0;__builtin_va_start(a,n);\
         for(i=0;i<n;i++){t+=__builtin_va_arg(a,int);t+=(int)__builtin_va_arg(a,double);}\
         return t;}\n\
         int main(void){return m(3,1,10.0,2,20.0,3,30.0);}",
        66,
    );
    // a long double variadic argument is a 16-byte quad in the VR area
    check(
        "double q(int n,...){__builtin_va_list a;double s=0;int i;__builtin_va_start(a,n);\
         for(i=0;i<n;i++)s+=(double)__builtin_va_arg(a,long double);return s;}\n\
         int main(void){return (int)q(3,1.5L,2.5L,3.5L);}",
        7,
    );
}

/// AAPCS64 §6.8.2 / §6.9 as C sees it: a composite argument and a composite
/// result mean the same thing whichever register file or stack slot they ride in.
#[test]
fn composites_by_value_and_by_return() {
    // ≤16 bytes: two general registers, out and back
    check(
        "struct P{int x,y;};struct P mk(int a){struct P p;p.x=a;p.y=a*2;return p;}\n\
         int sum(struct P p){return p.x+p.y;}\n\
         int main(void){return sum(mk(5));}",
        15,
    );
    // >16 bytes: the caller's copy, passed by reference, returned through x8
    check(
        "struct B{long a,b,c,d;};\n\
         struct B mk(long x){struct B r;r.a=x;r.b=x+1;r.c=x+2;r.d=x+3;return r;}\n\
         long sum(struct B s){return s.a+s.b+s.c+s.d;}\n\
         int main(void){return (int)(sum(mk(10))+sum(mk(100)));}",
        452,
    );
    // an HFA: four floats in v0–v3
    check(
        "struct H{float a,b,c,d;};\n\
         struct H mk(void){struct H h;h.a=1;h.b=2;h.c=3;h.d=4;return h;}\n\
         int sum(struct H h){return (int)(h.a+h.b+h.c+h.d);}\n\
         int main(void){return sum(mk());}",
        10,
    );
    // a composite pushed past the register file, onto the stack
    check(
        "struct P{int x,y;};\n\
         int f(int a,int b,int c,int d,int e,int f2,int g,struct P p){return a+p.x*100+p.y;}\n\
         int main(void){struct P p;p.x=3;p.y=4;return f(1,2,3,4,5,6,7,p);}",
        305,
    );
    // the value of a struct assignment, and a struct through ?: and a stmt-expr
    check(
        "struct P{int x,y;};\n\
         int main(void){struct P a,b;b.x=1;b.y=2;int c=(a=b).x;\n\
         struct P s={10,20},t={30,40};int d=(1?s:t).y;\n\
         int e=({struct P q={99,88};q;}).x;return c*10000+d*100+e;}",
        12099,
    );
}

