#include <stdio.h>
typedef struct{int err;long long v;}res;
static res halve(long long x){if(x&1)return (res){1,0};return (res){0,x>>1};}
static res bound(long long x){if(x>1000000000)return (res){2,0};return (res){0,x};}
static res step(long long x){res h=halve(x);if(h.err)return h;res b=bound(h.v*3);if(b.err)return b;return (res){0,b.v+1};}
int main(){long long okc=0,bad=0,t=0;for(long long i=0;i<20000000;i++){res r=step(i);if(!r.err){okc++;t+=r.v&255;}else bad++;}printf("%lld %lld %lld\n",okc,bad,t);return 0;}
