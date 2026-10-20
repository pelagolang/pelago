#include <stdio.h>
#include <stdlib.h>
int main(){long n=5000000;long*f=malloc((n+1)*sizeof(long));for(long i=0;i<=n;i++)f[i]=1;long c=0;
for(long i=2;i<=n;i++){if(f[i]==1){c++;long j=i*i;while(j<=n){f[j]=0;j+=i;}}}printf("%ld\n",c);return 0;}
