#include <cstdio>
#include <unordered_map>
int main(){std::unordered_map<long long,long long> m;for(long long i=0;i<200000;i++)m[i]=i*3;
long long t=0;for(long long i=0;i<200000;i++){auto it=m.find(i);if(it!=m.end())t+=it->second;}printf("200000 %lld\n",t);return 0;}
