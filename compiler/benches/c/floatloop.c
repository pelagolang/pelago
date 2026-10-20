#include <stdio.h>
int main(){double x=0.5,v=0.0,e=0.0;for(long i=0;i<20000000;i++){double a=-x*0.001;v+=a*0.01;x+=v*0.01;e+=x*x+v*v;}printf("%f\n",e);return 0;}
