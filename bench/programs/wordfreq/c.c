#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CAP 16384
typedef struct { char key[16]; long long n; int used; } slot;
static slot tab[CAP];
static unsigned long long fnv(const char*s){unsigned long long h=1469598103934665603ULL;while(*s){h^=(unsigned char)*s++;h*=1099511628211ULL;}return h;}
static slot*find(const char*k){unsigned long long h=fnv(k)&(CAP-1);while(tab[h].used&&strcmp(tab[h].key,k))h=(h+1)&(CAP-1);return &tab[h];}
int main(){long long distinct=0;char buf[16];
for(long long i=0;i<200000;i++){sprintf(buf,"w%lld",i*7919%5003);slot*s=find(buf);if(!s->used){s->used=1;strcpy(s->key,buf);distinct++;}s->n++;}
long long t=0;for(long long k=0;k<5003;k++){sprintf(buf,"w%lld",k);slot*s=find(buf);t+=(s->used?s->n:0)*(k&15);}
printf("%lld %lld\n",distinct,t);return 0;}
