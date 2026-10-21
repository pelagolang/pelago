#include <stdio.h>
enum{ADD,MUL,XOR,SHL,NEG,SKIP};
typedef struct{int k;long long n;}op;
static long long apply(long long acc,op o){switch(o.k){case ADD:return (acc+o.n)&1048575;case MUL:return (acc*o.n)&1048575;case XOR:return acc^o.n;case SHL:return (acc<<o.n)&1048575;case NEG:return 1048575-acc;default:return acc;}}
int main(){op prog[1024];long long s=7;
for(int i=0;i<1024;i++){s=(s*1103515245+12345)&2147483647;long long k=(s>>8)&255;int c=(int)((s>>20)%6);
switch(c){case 0:prog[i]=(op){ADD,k};break;case 1:prog[i]=(op){MUL,k+1};break;case 2:prog[i]=(op){XOR,k};break;case 3:prog[i]=(op){SHL,k&7};break;case 4:prog[i]=(op){NEG,0};break;default:prog[i]=(op){SKIP,0};}}
long long acc=1;for(int p=0;p<20000;p++)for(int i=0;i<1024;i++)acc=apply(acc,prog[i]);printf("%lld\n",acc);return 0;}
