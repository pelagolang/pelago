#include <stdio.h>
typedef struct{long long x,y,vx,vy;}body;
int main(){static body bs[1000];for(long long i=0;i<1000;i++)bs[i]=(body){i*7,i*13,(i&7)-3,(i&3)-1};
for(int s=0;s<20000;s++)for(int i=0;i<1000;i++){bs[i].x=(bs[i].x+bs[i].vx)&65535;bs[i].y=(bs[i].y+bs[i].vy)&65535;}
long long t=0;for(int i=0;i<1000;i++)t+=bs[i].x^bs[i].y;printf("%lld\n",t);return 0;}
