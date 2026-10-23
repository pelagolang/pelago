#include <stdio.h>
#include <stdlib.h>
int main(){long long n=200000;long long*adj=malloc(n*4*8),*dist=malloc(n*8),*queue=malloc(n*8);long long s=99;
for(long long i=0;i<n*4;i++){s=(s*1103515245+12345)&2147483647;adj[i]=s%n;}
for(long long i=0;i<n;i++){dist[i]=-1;queue[i]=0;}
long long head=0,tail=1;dist[0]=0;
while(head<tail){long long u=queue[head];head++;for(long long k=0;k<4;k++){long long v=adj[u*4+k];if(dist[v]<0){dist[v]=dist[u]+1;queue[tail]=v;tail++;}}}
long long t=0;for(long long i=0;i<n;i++)t+=dist[i]+1;printf("%lld %lld\n",tail,t);return 0;}
