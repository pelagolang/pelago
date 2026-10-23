#include <stdio.h>
int main(){long long bins[1024]={0};long long s=3;for(long long i=0;i<30000000;i++){s=(s*1103515245+12345)&2147483647;long long b=(s>>10)&1023;bins[b]=bins[b]+1;}
long long t=0;for(long long i=0;i<1024;i++)t+=bins[i]*(i&15);printf("%lld\n",t);return 0;}
