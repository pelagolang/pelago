#include <cstdio>
#include <vector>
int main(){std::vector<long long> xs;for(long long i=0;i<5000000;i++)xs.push_back(i);long long t=0;for(long long x:xs)t+=x;printf("%lld\n",t);return 0;}
