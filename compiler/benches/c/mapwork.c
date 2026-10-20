#include <stdio.h>
#include <stdlib.h>
#define N 524288
int main(){long*k=malloc(N*8),*v=malloc(N*8);char*u=calloc(N,1);for(long i=0;i<200000;i++){unsigned long h=(unsigned long)i*0x9E3779B97F4A7C15UL>>45;while(u[h]&&k[h]!=i)h=(h+1)&(N-1);u[h]=1;k[h]=i;v[h]=i*3;}
long t=0;for(long i=0;i<200000;i++){unsigned long h=(unsigned long)i*0x9E3779B97F4A7C15UL>>45;while(u[h]&&k[h]!=i)h=(h+1)&(N-1);if(u[h])t+=v[h];}printf("200000 %ld\n",t);return 0;}
