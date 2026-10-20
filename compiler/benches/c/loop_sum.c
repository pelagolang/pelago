#include <stdio.h>
int main(){long t=0;for(long i=0;i<300000000;i++)t+=i^(t&7);printf("%ld\n",t);return 0;}
