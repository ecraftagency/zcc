#include <stdio.h>
#define LIM 100000000
static char is[LIM+1];
int main(void){
  long i,j,count=0;
  for(i=2;i<=LIM;i++) is[i]=1;
  for(i=2;i*i<=LIM;i++)
    if(is[i])
      for(j=i*i;j<=LIM;j+=i) is[j]=0;
  for(i=2;i<=LIM;i++) if(is[i]) count++;
  printf("%ld\n",count);
  return 0;
}
