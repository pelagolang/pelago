#include <stdio.h>
int main(){long long n = 9;long long perm1[16],perm[16],count[16];for(long long i=0;i<n;i++){perm1[i]=i;perm[i]=0;count[i]=0;}
long long maxflips=0,checksum=0,permcount=0,r=n;int done=0;
while(!done){while(r!=1){count[r-1]=r;r--;}
for(long long i=0;i<n;i++)perm[i]=perm1[i];
long long flips=0,k=perm[0];
while(k!=0){long long i=0,j=k;while(i<j){long long t=perm[i];perm[i]=perm[j];perm[j]=t;i++;j--;}flips++;k=perm[0];}
if(flips>maxflips)maxflips=flips;
if(permcount%2==0)checksum+=flips;else checksum-=flips;
int more=1;while(more){if(r==n){done=1;more=0;}else{long long p0=perm1[0];long long i=0;while(i<r){perm1[i]=perm1[i+1];i++;}perm1[r]=p0;count[r]--;if(count[r]>0)more=0;else r++;}}
permcount++;}
printf("%lld %lld\n",checksum,maxflips);return 0;}
