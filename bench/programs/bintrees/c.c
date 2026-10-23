#include <stdio.h>
#include <stdlib.h>
typedef struct node { struct node *l, *r; } node;
static node *make(long long d){if(d==0)return NULL;node *n=malloc(sizeof(node));n->l=make(d-1);n->r=make(d-1);return n;}
static long long check(node *t){return t?1+check(t->l)+check(t->r):0;}
static void del(node *t){if(t){del(t->l);del(t->r);free(t);}}
int main(){long long total=0;for(long long it=0;it<20;it++){node *t=make(16);total+=check(t);del(t);}printf("%lld\n",total);return 0;}
