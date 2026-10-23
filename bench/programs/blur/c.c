#include <stdio.h>
#include <stdlib.h>
int main(){long long n=1024;long long*a=malloc(n*n*8),*b=malloc(n*n*8);long long s=5;
for(long long i=0;i<n*n;i++){s=(s*1103515245+12345)&2147483647;a[i]=s&255;b[i]=0;}
for(long long pass=0;pass<4;pass++){
for(long long y=1;y<n-1;y++)for(long long x=1;x<n-1;x++){long long i=y*n+x;b[i]=(a[i-n-1]+a[i-n]+a[i-n+1]+a[i-1]+a[i]+a[i+1]+a[i+n-1]+a[i+n]+a[i+n+1])/9;}
for(long long i=n;i<n*n-n;i++)a[i]=b[i];}
long long t=0;for(long long i=0;i<n*n;i++)t+=a[i]*(i&7);printf("%lld\n",t);return 0;}
