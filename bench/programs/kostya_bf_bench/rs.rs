// Brainfuck interpreter after kostya/benchmarks brainfuck/bf.c (MIT, Copyright (c) 2014 Konstantin Makarchev); program: brainfuck/bench.b
enum Op {
    Inc(i64),
    Move(i64),
    Loop(Vec<Op>),
    Print,
}

fn parse(code: &[u8], pos: &mut usize) -> Vec<Op> {
    let mut ops = Vec::new();
    while *pos < code.len() {
        let c = code[*pos];
        *pos += 1;
        match c {
            b'+' => ops.push(Op::Inc(1)),
            b'-' => ops.push(Op::Inc(-1)),
            b'>' => ops.push(Op::Move(1)),
            b'<' => ops.push(Op::Move(-1)),
            b'.' => ops.push(Op::Print),
            b'[' => {
                let body = parse(code, pos);
                ops.push(Op::Loop(body));
            }
            b']' => return ops,
            _ => {}
        }
    }
    ops
}

fn eval(ops: &[Op], tape: &mut Vec<i64>, pos: &mut usize, sum1: &mut i64, sum2: &mut i64) {
    for o in ops {
        match o {
            Op::Inc(d) => tape[*pos] += d,
            Op::Move(d) => {
                *pos = (*pos as i64 + d) as usize;
                if *pos >= tape.len() {
                    let n = tape.len();
                    tape.resize(n * 2, 0);
                }
            }
            Op::Loop(body) => {
                while tape[*pos] > 0 {
                    eval(body, tape, pos, sum1, sum2);
                }
            }
            Op::Print => {
                *sum1 = (*sum1 + tape[*pos]) % 255;
                *sum2 = (*sum2 + *sum1) % 255;
            }
        }
    }
}

fn main() {
    let src = ">++[<+++++++++++++>-]<[[>+>+<<-]>[<+>-]++++++++[>++++++++<-]>.[-]<<>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[>++++++++++[-]<-]<-]<-]<-]<-]<-]<-]++++++++++.";
    let mut pos = 0;
    let ops = parse(src.as_bytes(), &mut pos);
    let mut tape = vec![0i64];
    let mut p = 0usize;
    let (mut sum1, mut sum2) = (0i64, 0i64);
    eval(&ops, &mut tape, &mut p, &mut sum1, &mut sum2);
    println!("{}", (sum2 << 8) | sum1);
}
