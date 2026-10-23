#include <algorithm>
#include <cstdio>
#include <vector>
int main(){long long n=1000000;std::vector<long long> xs(n);long long s=12345;for(long long i=0;i<n;i++){s=(s*1103515245+12345)&2147483647;xs[i]=s;}
std::sort(xs.begin(),xs.end());long long t=0;for(long long i=0;i<n;i++)t+=(xs[i]&255)*(i&15);printf("%lld %lld %lld\n",xs[0],xs[n-1],t);return 0;}
