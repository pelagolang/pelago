enum Tree { Leaf, Node(Box<Tree>, Box<Tree>) }
fn make(d: i64) -> Tree { if d == 0 { Tree::Leaf } else { Tree::Node(Box::new(make(d - 1)), Box::new(make(d - 1))) } }
fn check(t: &Tree) -> i64 { match t { Tree::Leaf => 0, Tree::Node(l, r) => 1 + check(l) + check(r) } }
fn main() { let mut total = 0; for _ in 0..20 { total += check(&make(16)); } println!("{}", total); }
