// Expert tier: one bump arena per tree, no per-node malloc or free.
#include <cstdio>
#include <vector>
struct Node { Node *l, *r; };
static std::vector<Node> arena;
static Node *make(long long d){if(d==0)return nullptr;Node*n=&arena.emplace_back();n->l=make(d-1);n->r=make(d-1);return n;}
static long long check(const Node*t){return t?1+check(t->l)+check(t->r):0;}
int main(){arena.reserve(1<<20);long long total=0;for(long long it=0;it<20;it++){arena.clear();total+=check(make(16));}printf("%lld\n",total);return 0;}
