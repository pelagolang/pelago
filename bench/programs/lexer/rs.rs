fn main() {
    let mut text = String::new();
    for i in 0..60000i64 { text.push_str(&format!("let x{} = 12{} + foo(y) ;\n", i % 97, i % 13)); }
    let (mut idents, mut numbers, mut symbols) = (0i64, 0i64, 0i64);
    let (mut in_ident, mut in_number) = (false, false);
    for c in text.chars() {
        let letter = c.is_ascii_lowercase() || c == '_'; let digit = c.is_ascii_digit();
        if letter && !in_number { if !in_ident { idents += 1; } in_ident = true; }
        else if digit && !in_ident { if !in_number { numbers += 1; } in_number = true; }
        else if digit { in_number = false; }
        else { in_ident = false; in_number = false; if c != ' ' && c != '\n' { symbols += 1; } }
    }
    println!("{} {} {}", idents, numbers, symbols);
}
