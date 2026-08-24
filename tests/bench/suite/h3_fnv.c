#include <stdio.h>
unsigned long fnv(const unsigned char *p,int n){ unsigned long h=1469598103934665603ul; int i; for(i=0;i<n;i++){ h ^= p[i]; h *= 1099511628211ul; } return h; }
static unsigned char buf[4096];
int main(void){ int i; for(i=0;i<4096;i++) buf[i]=(unsigned char)(i*31+7); unsigned long s=0; long k; for(k=0;k<20000;k++){ buf[0]=(unsigned char)k; s ^= fnv(buf,4096); } printf("%lu\n", s); return 0; }
