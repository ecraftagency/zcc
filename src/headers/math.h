#ifndef _MATH_H
#define _MATH_H
#define HUGE_VAL 1e999 /* overflow khi lex → +inf thật (strtod trả inf, phải so == được) */
#define M_PI 3.14159265358979323846 /* EXT(gcc): POSIX, redis geohash đòi */
/* EXT(c99): INFINITY/NAN — redis hyperloglog, rdb, t_zset đòi */
#define INFINITY HUGE_VAL
#define NAN (0.0 / 0.0)
/* EXT(c99): phân loại float — đủ cho redis; subnormal tính là NORMAL */
#define FP_NAN 1
#define FP_INFINITE 2
#define FP_ZERO 3
#define FP_NORMAL 4
#define FP_SUBNORMAL 5
#define isnan(x) ((x) != (x))
#define isinf(x) ((x) == HUGE_VAL || (x) == -HUGE_VAL)
#define isfinite(x) (!isnan(x) && !isinf(x))
#define fpclassify(x) \
    (isnan(x) ? FP_NAN : (x) == 0.0 ? FP_ZERO : isinf(x) ? FP_INFINITE : FP_NORMAL)
double sin(double);
double cos(double);
double tan(double);
double asin(double);
double acos(double);
double atan(double);
double atan2(double, double);
double sinh(double);
double cosh(double);
double tanh(double);
double exp(double);
double log(double);
double log10(double);
double pow(double, double);
double sqrt(double);
double ceil(double);
double floor(double);
double fabs(double);
double fmod(double, double);
double ldexp(double, int);
long double ldexpl(long double, int);
double frexp(double, int *);
double modf(double, double *);
#endif
