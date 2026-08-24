int sum(int*a,int n){ int s=0; for(int i=0;i<n;i++) s+=a[i]*2+i; return s; }
long fact(int n){ long r=1; while(n>1){ r*=n; n--; } return r; }
int nest(int n){ int t=0; for(int i=0;i<n;i++) for(int j=0;j<n;j++) t+=i*j; return t; }
