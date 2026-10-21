#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(){size_t cap=4000000,len=0;char*s=malloc(cap);
for(long long i=0;i<300000;i++){len+=sprintf(s+len,"%lld ",i);}
long long t=0;size_t i=0;while(i<len){size_t j=i;long long v=0;while(j<len&&s[j]!=' '){v=v*10+(s[j]-'0');j++;}if(j>i)t+=v&1023;i=j+1;}
printf("%zu %lld\n",len,t);return 0;}
