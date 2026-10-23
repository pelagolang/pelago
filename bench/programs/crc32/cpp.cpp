#include <stdio.h>
int main(){long long table[256];for(long long n=0;n<256;n++){long long c=n;for(int k=0;k<8;k++){if((c&1)==1)c=3988292384LL^(c>>1);else c=c>>1;}table[n]=c;}
long long crc=4294967295LL,s=1;for(long long i=0;i<20000000;i++){s=(s*1103515245+12345)&2147483647;crc=table[(crc^(s>>8))&255]^(crc>>8);}
printf("%lld\n",crc^4294967295LL);return 0;}
