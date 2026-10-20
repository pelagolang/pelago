#include <stdio.h>
typedef struct{long x,y;}vec;
static vec add(vec a,vec o){vec r={a.x+o.x,a.y+o.y};return r;}
static long dot(vec a,vec o){return a.x*o.x+a.y*o.y;}
int main(){vec p={1,2};vec d={3,1};long t=0;for(long i=0;i<10000000;i++){p=add(p,d);t+=dot(p,d)&1023;vec q={p.x&65535,p.y&65535};p=q;}printf("%ld\n",t);return 0;}
