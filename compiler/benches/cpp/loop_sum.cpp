#include <stdio.h>
int main(){long long t=0;for(long long i=0;i<300000000;i++)t+=i^(t&7);printf("%lld\n",t);return 0;}
