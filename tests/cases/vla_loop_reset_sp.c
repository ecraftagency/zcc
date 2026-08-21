#include <stdio.h>
int sum(int n){
  int a[n];               /* VLA */
  int i, s=0;
  for(i=0;i<n;i++) a[i]=i*i;
  for(i=0;i<n;i++) s+=a[i];
  return s;
}
int loopvla(int n){       /* VLA cấp lại mỗi vòng — goto-lùi phải reset sp */
  int total=0, k;
  for(k=1;k<=n;k++){
    char buf[k*8];
    int j; for(j=0;j<k*8;j++) buf[j]=(char)j;
    total += buf[k];
  }
  return total;
}
int main(void){
  printf("%d %d\n", sum(10), loopvla(20));
  return 0;
}
