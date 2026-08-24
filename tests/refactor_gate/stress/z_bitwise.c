unsigned f(unsigned a, unsigned b){ return (a&b)|(a^b)|(a<<3)|(b>>2)|(~a); }
long g(long a, long b){ return (a&0xff)|(b<<12)|(a>>7)|(a%b)|(a/b); }
int sh(int x, int n){ return (x<<n) + (x>>n) + (x&15) + (x|7) + (x^3); }
