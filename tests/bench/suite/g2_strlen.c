#include <stdio.h>
static char buf[100000];
long mystrlen(const char *s){ const char *p=s; while(*p) p++; return p-s; }
int main(void){ int i; for(i=0;i<99999;i++) buf[i]='a'+(i&15); buf[99999]=0; long s=0,k; for(k=0;k<5000;k++){ buf[(int)(k&1023)]=(char)('a'+(s&7)); s += mystrlen(buf); } printf("%ld\n",s); return 0; }
