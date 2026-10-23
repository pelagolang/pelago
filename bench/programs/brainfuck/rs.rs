fn main() {
    let prog = "-[>-[>-[>+<-]<-]<-]>>>.";
    let code: Vec<i64> = prog
        .chars()
        .map(|c| match c {
            '+' => 0,
            '-' => 1,
            '<' => 2,
            '>' => 3,
            '[' => 4,
            ']' => 5,
            _ => 6,
        })
        .collect();
    let mut jump = vec![0usize; code.len()];
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..code.len() {
        if code[i] == 4 {
            stack.push(i);
        } else if code[i] == 5 {
            let open = stack.pop().unwrap();
            jump[open] = i;
            jump[i] = open;
        }
    }
    let mut tape = vec![0i64; 30000];
    let (mut ptr, mut pc, mut steps, mut out) = (0usize, 0usize, 0i64, 0i64);
    while pc < code.len() {
        let op = code[pc];
        steps += 1;
        match op {
            0 => tape[ptr] = (tape[ptr] + 1) & 255,
            1 => tape[ptr] = (tape[ptr] + 255) & 255,
            2 => ptr -= 1,
            3 => ptr += 1,
            4 => {
                if tape[ptr] == 0 {
                    pc = jump[pc];
                }
            }
            5 => {
                if tape[ptr] != 0 {
                    pc = jump[pc];
                }
            }
            _ => out += tape[ptr],
        }
        pc += 1;
    }
    let sum: i64 = tape.iter().sum();
    println!("{} {} {}", steps, sum, out);
}
