struct B { unsigned a:5, b:7, c:20; };
int f(int n){ struct B s; s.a=1; s.b=2; s.c=3; s.a+=n; s.b+=s.a; s.c+=s.b; return s.a*1000+s.b*100+s.c; }
