int m(int a,int b,int c,int d,int e,int f,int g,int h,int i,int j){
 return a+b+c+d+e+f+g+h+i+j; }
struct P { long x,y,z; };
long sv(struct P p){ return p.x+p.y+p.z; }
long call(void){ struct P p={1,2,3}; return sv(p) + m(1,2,3,4,5,6,7,8,9,10); }
