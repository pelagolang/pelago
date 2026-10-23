#include <cstdio>
#include <vector>
int main(){long long n=5000000;std::vector<long long> f(n+1,1);long long c=0;
for(long long i=2;i<=n;i++){if(f[i]==1){c++;long long j=i*i;while(j<=n){f[j]=0;j+=i;}}}printf("%lld\n",c);return 0;}
