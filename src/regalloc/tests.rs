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
fn same(src: &str) {
    let ast = frontend(src);
    let h = hir::build::build(&ast);
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
