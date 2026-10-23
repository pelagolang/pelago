#include <stdio.h>
#include <stdlib.h>
int main(){long long cap=1,n=0;long long*xs=malloc(8);for(long long i=0;i<5000000;i++){if(n==cap){cap*=2;xs=realloc(xs,cap*8);}xs[n++]=i;}long long t=0;for(long long i=0;i<n;i++)t+=xs[i];printf("%lld\n",t);return 0;}
