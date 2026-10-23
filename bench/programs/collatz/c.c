#include <stdio.h>
long long steps(long long n){long long s=0;while(n!=1){if(n%2==0)n/=2;else n=3*n+1;s++;}return s;}
int main(){long long t=0;for(long long i=1;i<300001;i++)t+=steps(i);printf("%lld\n",t);}
