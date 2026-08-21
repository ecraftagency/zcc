/* The asm surface musl requires: pinned reg, tied "0", "w" FP, "Q" memory */
int printf(const char *, ...);
static long add_pin(long a, long b) {
	register long x9 __asm__("x9") = a;
	register long x10 __asm__("x10") = b;
	register long x11 __asm__("x11");
	__asm__ __volatile__("add %0, %1, %2" : "=r"(x11) : "r"(x9), "0"(x10));
	/* "0"(x10): input tied to the reg of output 0 (x11) — add x11,x9,x11 */
	return x11;
}
static double fabs_w(double x) {
	double r;
	__asm__("fabs %d0, %d1" : "=w"(r) : "w"(x));
	return r;
}
static float fmaxf_w(float a, float b) {
	float r;
	__asm__("fmax %s0, %s1, %s2" : "=w"(r) : "w"(a), "w"(b));
	return r;
}
static int ll_sc(volatile int *p, int v) {
	/* LL/SC must live ENTIRELY within one template: at -O0 every compiler
	   (cc included) inserts a stack store between two asm blocks -> clears the
	   exclusive monitor -> livelock */
	int old, flag;
	__asm__ __volatile__("1:\n"
			     "ldaxr %w0, %2\n"
			     "stlxr %w1, %w3, %2\n"
			     "cbnz %w1, 1b"
			     : "=&r"(old), "=&r"(flag), "+Q"(*p)
			     : "r"(v)
			     : "memory");
	return old;
}
int main(void) {
	volatile int cell = 5;
	printf("add=%ld\n", add_pin(30, 12));
	printf("fabs=%.1f fmax=%.1f\n", fabs_w(-3.5), (double) fmaxf_w(1.5f, 7.5f));
	printf("old=%d new=%d\n", ll_sc(&cell, 42), cell);
	return 0;
}
