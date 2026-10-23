#include <cstdio>
#include <vector>
int main(){const long long cap=1024,space=8192;std::vector<long long> key(cap,-1),prev(cap,-1),next(cap,-1),slot_of(space,-1);
long long head=-1,tail=-1,size=0,hits=0,s=17;
for(long long i=0;i<5000000;i++){s=(s*1103515245+12345)&2147483647;long long r=(s>>8)%space;long long k=(s&3)==0?r%512:r;long long slot=slot_of[k];
if(slot>=0){hits++;if(slot!=head){long long p=prev[slot],n=next[slot];next[p]=n;if(n>=0)prev[n]=p;else tail=p;prev[slot]=-1;next[slot]=head;prev[head]=slot;head=slot;}}
else{if(size<cap){slot=size;size++;}else{slot=tail;slot_of[key[slot]]=-1;tail=prev[slot];if(tail>=0)next[tail]=-1;else head=-1;}
key[slot]=k;slot_of[k]=slot;prev[slot]=-1;next[slot]=head;if(head>=0)prev[head]=slot;else tail=slot;head=slot;}}
printf("%lld %lld\n",hits,head);return 0;}
