#include <stdio.h>
int main(){long long size = 400;long long total=0;
for(long long py=0;py<size;py++)for(long long px=0;px<size;px++){long long cx=(px*3500)/size-2500,cy=(py*2000)/size-1000,x=0,y=0,it=0;
while(it<100&&(x*x+y*y)<4000000){long long nx=(x*x-y*y)/1000+cx;y=(2*x*y)/1000+cy;x=nx;it++;}total+=it;}
printf("%lld\n",total);return 0;}
