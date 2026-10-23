#include <cstdio>
#include <memory>
#include <variant>
struct Num { long long n; };
struct X {};
struct Add; struct Mul; struct Neg;
using Expr = std::variant<Num, X, std::unique_ptr<Add>, std::unique_ptr<Mul>, std::unique_ptr<Neg>>;
struct Add { Expr a, b; };
struct Mul { Expr a, b; };
struct Neg { Expr a; };
static Expr build(long long d,long long s){
    if(d==0){if(s&1)return X{};return Num{s&15};}
    long long l=(s*1103515245+12345)&2147483647,r=(s*69069+1)&2147483647;
    switch(s%3){
    case 0:{auto n=std::make_unique<Add>();n->a=build(d-1,l);n->b=build(d-1,r);return n;}
    case 1:{auto n=std::make_unique<Mul>();n->a=build(d-1,l);n->b=build(d-1,r);return n;}
    default:{auto n=std::make_unique<Neg>();n->a=build(d-1,l);return n;}
    }
}
static long long eval(const Expr&e,long long v){
    struct V {
        long long v;
        long long operator()(const Num&n)const{return n.n;}
        long long operator()(const X&)const{return v&1048575;}
        long long operator()(const std::unique_ptr<Add>&n)const{return (eval(n->a,v)+eval(n->b,v))&1048575;}
        long long operator()(const std::unique_ptr<Mul>&n)const{return (eval(n->a,v)*eval(n->b,v))&1048575;}
        long long operator()(const std::unique_ptr<Neg>&n)const{return 1048575-eval(n->a,v);}
    };
    return std::visit(V{v},e);
}
int main(){Expr e=build(14,7);long long t=0;for(long long v=0;v<3000;v++)t=(t+eval(e,v))&1048575;printf("%lld\n",t);return 0;}
