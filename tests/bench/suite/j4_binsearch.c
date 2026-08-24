#include <stdio.h>
static int a[100000];
int bsearch_i(int *p,int n,int key){ int lo=0,hi=n-1; while(lo<=hi){ int mid=(lo+hi)>>1; if(p[mid]==key) return mid; if(p[mid]<key) lo=mid+1; else hi=mid-1; } return -1; }
int main(void){ int i; for(i=0;i<100000;i++) a[i]=i*2; long s=0,k; for(k=0;k<400000;k++) s += bsearch_i(a,100000,(int)((k*7)%200000)); printf("%ld\n",s); return 0; }
