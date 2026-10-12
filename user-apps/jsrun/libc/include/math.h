/* Kitsune jsrun libc shim: <math.h>. Double precision only; the functions are musl's libm
   (../../vendor/musl-math) plus a few in ../libc.c (ceil, sqrt, fabs). Classification macros use the
   compiler builtins. */
#ifndef KJS_MATH_H
#define KJS_MATH_H

typedef double double_t;
typedef float float_t;

#define NAN __builtin_nanf("")
#define INFINITY __builtin_inff()
#define HUGE_VAL __builtin_huge_val()
#define HUGE_VALF __builtin_huge_valf()
#define HUGE_VALL __builtin_huge_vall()

#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4

#define MATH_ERRNO 1
#define MATH_ERREXCEPT 2
#define math_errhandling 2

#define fpclassify(x) __builtin_fpclassify(FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO, x)
#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define isnormal(x) __builtin_isnormal(x)
#define signbit(x) __builtin_signbit(x)
#define isgreater(x, y) __builtin_isgreater(x, y)
#define isgreaterequal(x, y) __builtin_isgreaterequal(x, y)
#define isless(x, y) __builtin_isless(x, y)
#define islessequal(x, y) __builtin_islessequal(x, y)
#define islessgreater(x, y) __builtin_islessgreater(x, y)
#define isunordered(x, y) __builtin_isunordered(x, y)

#define M_E 2.7182818284590452354
#define M_LOG2E 1.4426950408889634074
#define M_LOG10E 0.43429448190325182765
#define M_LN2 0.69314718055994530942
#define M_LN10 2.30258509299404568402
#define M_PI 3.14159265358979323846
#define M_PI_2 1.57079632679489661923
#define M_PI_4 0.78539816339744830962
#define M_1_PI 0.31830988618379067154
#define M_2_PI 0.63661977236758134308
#define M_2_SQRTPI 1.12837916709551257390
#define M_SQRT2 1.41421356237309504880
#define M_SQRT1_2 0.70710678118654752440

int __fpclassify(double x);
int __signbit(double x);

double acos(double x);
double acosh(double x);
double asin(double x);
double asinh(double x);
double atan(double x);
double atan2(double y, double x);
double atanh(double x);
double cbrt(double x);
double ceil(double x);
double copysign(double x, double y);
double cos(double x);
double cosh(double x);
double exp(double x);
double exp10(double x);
double exp2(double x);
double expm1(double x);
double fabs(double x);
double fdim(double x, double y);
double floor(double x);
double fma(double x, double y, double z);
double fmax(double x, double y);
double fmin(double x, double y);
double fmod(double x, double y);
double frexp(double x, int *e);
double hypot(double x, double y);
int ilogb(double x);
double ldexp(double x, int e);
double log(double x);
double log10(double x);
double log1p(double x);
double log2(double x);
double logb(double x);
long lrint(double x);
long long llrint(double x);
long lround(double x);
long long llround(double x);
double modf(double x, double *ip);
double nan(const char *tag);
double nearbyint(double x);
double nextafter(double x, double y);
double pow(double x, double y);
double remainder(double x, double y);
double remquo(double x, double y, int *q);
double rint(double x);
double round(double x);
double scalbln(double x, long n);
double scalbn(double x, int n);
double sin(double x);
void sincos(double x, double *s, double *c);
double sinh(double x);
double sqrt(double x);
double tan(double x);
double tanh(double x);
double trunc(double x);

float fabsf(float x);
float sqrtf(float x);

#endif
