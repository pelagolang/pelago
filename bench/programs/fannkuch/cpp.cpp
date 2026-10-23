#include <cstdio>
#include <utility>
#include <vector>
int main(){long long n = 9;std::vector<long long> perm1(n),perm(n,0),count(n,0);for(long long i=0;i<n;i++)perm1[i]=i;
long long maxflips=0,checksum=0,permcount=0,r=n;bool done=false;
while(!done){while(r!=1){count[r-1]=r;r--;}
perm=perm1;long long flips=0,k=perm[0];
while(k!=0){long long i=0,j=k;while(i<j){std::swap(perm[i],perm[j]);i++;j--;}flips++;k=perm[0];}
if(flips>maxflips)maxflips=flips;
if(permcount%2==0)checksum+=flips;else checksum-=flips;
bool more=true;while(more){if(r==n){done=true;more=false;}else{long long p0=perm1[0];long long i=0;while(i<r){perm1[i]=perm1[i+1];i++;}perm1[r]=p0;count[r]--;if(count[r]>0)more=false;else r++;}}
permcount++;}
printf("%lld %lld\n",checksum,maxflips);return 0;}
