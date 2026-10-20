#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(){char*s=malloc(1);s[0]=0;long len=0,t=0;for(long i=0;i<20000;i++){char*n=malloc(len+3);memcpy(n,s,len);n[len]='a';n[len+1]='b';n[len+2]=0;free(s);s=n;len+=2;t+=len&7;}printf("%ld %ld\n",len,t);return 0;}
