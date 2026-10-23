#include <cstdio>
#include <memory>
struct Node { std::unique_ptr<Node> l, r; };
static std::unique_ptr<Node> make(long long d){if(d==0)return nullptr;auto n=std::make_unique<Node>();n->l=make(d-1);n->r=make(d-1);return n;}
static long long check(const Node*t){return t?1+check(t->l.get())+check(t->r.get()):0;}
int main(){long long total=0;for(long long it=0;it<20;it++){auto t=make(16);total+=check(t.get());}printf("%lld\n",total);return 0;}
