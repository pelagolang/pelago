#include <stdio.h>
#include <stdlib.h>
typedef struct node { int kind; long long n; struct node *a, *b; } node; /* 0 num, 1 x, 2 add, 3 mul, 4 neg */
static node *mk(int kind,long long n,node*a,node*b){node*e=malloc(sizeof(node));e->kind=kind;e->n=n;e->a=a;e->b=b;return e;}
static node *build(long long d,long long s){
    if(d==0)return (s&1)==1?mk(1,0,NULL,NULL):mk(0,s&15,NULL,NULL);
    long long l=(s*1103515245+12345)&2147483647,r=(s*69069+1)&2147483647;
    switch(s%3){
    case 0:{node*a=build(d-1,l);node*b=build(d-1,r);return mk(2,0,a,b);}
    case 1:{node*a=build(d-1,l);node*b=build(d-1,r);return mk(3,0,a,b);}
    default:{node*a=build(d-1,l);return mk(4,0,a,NULL);}
    }
}
static long long eval(node*e,long long v){
    switch(e->kind){
    case 0:return e->n;
    case 1:return v&1048575;
    case 2:return (eval(e->a,v)+eval(e->b,v))&1048575;
    case 3:return (eval(e->a,v)*eval(e->b,v))&1048575;
    default:return 1048575-eval(e->a,v);
    }
}
int main(){node*e=build(14,7);long long t=0;for(long long v=0;v<3000;v++)t=(t+eval(e,v))&1048575;printf("%lld\n",t);return 0;}
