#include <stdio.h>
#include <stdlib.h>
#include <string.h>
/* Amortised growth: capacity doubles, so appending is linear overall. */
int main(){long long cap=16,len=0,t=0;char*s=malloc(cap);for(long long i=0;i<2000000;i++){if(len+2>cap){cap*=2;s=realloc(s,cap);}s[len]='a';s[len+1]='b';len+=2;t+=len&7;}printf("%lld %lld\n",len,t);return 0;}
