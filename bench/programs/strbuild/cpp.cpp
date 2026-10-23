#include <cstdio>
#include <string>
int main(){std::string s;long long t=0;for(long long i=0;i<2000000;i++){s.push_back('a');s.push_back('b');t+=(long long)s.size()&7;}printf("%lld %lld\n",(long long)s.size(),t);return 0;}
