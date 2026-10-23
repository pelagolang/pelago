#include <cstdio>
#include <string>
int main(){std::string text;
for(long long i=0;i<60000;i++){text+="let x"+std::to_string(i%97)+" = 12"+std::to_string(i%13)+" + foo(y) ;\n";}
long long idents=0,numbers=0,symbols=0;bool in_ident=false,in_number=false;
for(char c:text){bool letter=(c>='a'&&c<='z')||c=='_';bool digit=c>='0'&&c<='9';
if(letter&&!in_number){if(!in_ident)idents++;in_ident=true;}
else if(digit&&!in_ident){if(!in_number)numbers++;in_number=true;}
else if(digit){in_number=false;}
else{in_ident=false;in_number=false;if(c!=' '&&c!='\n')symbols++;}}
printf("%lld %lld %lld\n",idents,numbers,symbols);return 0;}
