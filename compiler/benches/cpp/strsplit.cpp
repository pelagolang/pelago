#include <cstdio>
#include <string>
int main(){std::string s;for(long long i=0;i<300000;i++){s+=std::to_string(i);s+=' ';}
long long t=0;size_t len=s.size(),i=0;while(i<len){size_t j=i;long long v=0;while(j<len&&s[j]!=' '){v=v*10+(s[j]-'0');j++;}if(j>i)t+=v&1023;i=j+1;}
printf("%zu %lld\n",len,t);return 0;}
