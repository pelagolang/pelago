#include <stdio.h>
long long tak(long long x,long long y,long long z){return y<x?tak(tak(x-1,y,z),tak(y-1,z,x),tak(z-1,x,y)):z;}
int main(){printf("%lld\n",tak(30,20,10));return 0;}
