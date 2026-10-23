#include <cstdio>
#include <vector>
int main(){long long n=200000;std::vector<long long> adj,dist(n,-1),queue(n,0);long long s=99;
for(long long i=0;i<n*4;i++){s=(s*1103515245+12345)&2147483647;adj.push_back(s%n);}
long long head=0,tail=1;dist[0]=0;
while(head<tail){long long u=queue[head];head++;for(long long k=0;k<4;k++){long long v=adj[u*4+k];if(dist[v]<0){dist[v]=dist[u]+1;queue[tail]=v;tail++;}}}
long long t=0;for(long long i=0;i<n;i++)t+=dist[i]+1;printf("%lld %lld\n",tail,t);return 0;}
