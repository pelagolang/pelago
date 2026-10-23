#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(){size_t cap=60000*64+64,len=0;char*text=malloc(cap);
for(long long i=0;i<60000;i++)len+=sprintf(text+len,"let x%lld = 12%lld + foo(y) ;\n",i%97,i%13);
long long idents=0,numbers=0,symbols=0;int in_ident=0,in_number=0;
for(size_t p=0;p<len;p++){char c=text[p];int letter=(c>='a'&&c<='z')||c=='_';int digit=c>='0'&&c<='9';
if(letter&&!in_number){if(!in_ident)idents++;in_ident=1;}
else if(digit&&!in_ident){if(!in_number)numbers++;in_number=1;}
else if(digit){in_number=0;}
else{in_ident=0;in_number=0;if(c!=' '&&c!='\n')symbols++;}}
printf("%lld %lld %lld\n",idents,numbers,symbols);return 0;}
