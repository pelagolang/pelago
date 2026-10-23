#include <cstdio>
#include <string>
#include <unordered_map>
int main(){std::unordered_map<std::string,long long> counts;
for(long long i=0;i<200000;i++)counts["w"+std::to_string(i*7919%5003)]++;
long long t=0;for(long long k=0;k<5003;k++){auto it=counts.find("w"+std::to_string(k));t+=(it==counts.end()?0:it->second)*(k&15);}
printf("%zu %lld\n",counts.size(),t);return 0;}
