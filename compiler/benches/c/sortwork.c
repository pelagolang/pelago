#include <stdio.h>
#include <stdlib.h>
static int cmp(const void*a,const void*b){long long x=*(const long long*)a,y=*(const long long*)b;return x<y?-1:x>y;}
int main(){long long n=1000000;long long*xs=malloc(n*sizeof(long long));long long s=12345;for(long long i=0;i<n;i++){s=(s*1103515245+12345)&2147483647;xs[i]=s;}
qsort(xs,n,sizeof(long long),cmp);long long t=0;for(long long i=0;i<n;i++)t+=(xs[i]&255)*(i&15);printf("%lld %lld %lld\n",xs[0],xs[n-1],t);return 0;}
