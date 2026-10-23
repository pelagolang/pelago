// Expert tier: fixed open-addressing table with inline keys, no allocation per word.
#include <cstdio>
#include <cstring>
struct Slot { char key[16]; long long n; bool used; };
static Slot tab[16384];
static unsigned long long fnv(const char*s){unsigned long long h=1469598103934665603ULL;while(*s){h^=(unsigned char)*s++;h*=1099511628211ULL;}return h;}
static Slot*find(const char*k){unsigned long long h=fnv(k)&16383;while(tab[h].used&&strcmp(tab[h].key,k))h=(h+1)&16383;return &tab[h];}
int main(){long long distinct=0;char buf[16];
for(long long i=0;i<200000;i++){snprintf(buf,16,"w%lld",i*7919%5003);Slot*s=find(buf);if(!s->used){s->used=true;strcpy(s->key,buf);distinct++;}s->n++;}
long long t=0;for(long long k=0;k<5003;k++){snprintf(buf,16,"w%lld",k);Slot*s=find(buf);t+=(s->used?s->n:0)*(k&15);}
printf("%lld %lld\n",distinct,t);return 0;}
