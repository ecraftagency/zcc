int printf(const char *, ...);
int main(void) {
	double a = 0x1p54, b = 0x1.71547652b82fep+0, c = 0x1p-1074;
	double d = 0x.8p1, e = 0x1.fffffffffffffp+1023;
	float f = 0x1p-149f, g = 0x1.8p3f;
	printf("%a %a %a\n%a %a\n%a %a\n", a, b, c, d, e, (double) f, (double) g);
	return 0;
}
