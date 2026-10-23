#include <cstdio>
#include <vector>
int main(){long long n=256;std::vector<long long> a(n*n),b(n*n),c(n*n,0);
for(long long i=0;i<n*n;i++){a[i]=(i*7)&15;b[i]=(i*13+3)&15;}
for(long long i=0;i<n;i++)for(long long j=0;j<n;j++){long long sum=0;for(long long k=0;k<n;k++)sum+=a[i*n+k]*b[k*n+j];c[i*n+j]=sum;}
long long t=0;for(long long i=0;i<n*n;i++)t+=c[i]*(i&7);printf("%lld\n",t);return 0;}
