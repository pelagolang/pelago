// After kostya/benchmarks json (MIT, Copyright (c) 2014 Konstantin Makarchev): parse pretty-printed JSON of coordinates and average x, y, z. The text is built in memory, as byte codes, because the test's /tmp/1.json cannot be read yet. Keys are kept as hashes.
enum Jv {
    Lit,
    Num(f64),
    Str(i64),
    Arr(Vec<Jv>),
    Dict(Vec<i64>, Vec<Jv>),
}

fn skip(t: &[i64], p: &mut usize) {
    while *p < t.len() && (t[*p] == 32 || t[*p] == 10 || t[*p] == 13 || t[*p] == 9) {
        *p += 1;
    }
}

fn parse_value(t: &[i64], p: &mut usize) -> Jv {
    skip(t, p);
    let c = t[*p];
    if c == 123 {
        *p += 1;
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        skip(t, p);
        if t[*p] == 125 {
            *p += 1;
            return Jv::Dict(keys, vals);
        }
        let mut more = true;
        while more {
            skip(t, p);
            *p += 1;
            let mut h = 0i64;
            while t[*p] != 34 {
                h = (h * 31 + t[*p]) & 2147483647;
                *p += 1;
            }
            *p += 1;
            skip(t, p);
            *p += 1;
            let v = parse_value(t, p);
            keys.push(h);
            vals.push(v);
            skip(t, p);
            if t[*p] == 44 {
                *p += 1;
            } else {
                *p += 1;
                more = false;
            }
        }
        Jv::Dict(keys, vals)
    } else if c == 91 {
        *p += 1;
        let mut items = Vec::new();
        skip(t, p);
        if t[*p] == 93 {
            *p += 1;
            return Jv::Arr(items);
        }
        let mut more = true;
        while more {
            let v = parse_value(t, p);
            items.push(v);
            skip(t, p);
            if t[*p] == 44 {
                *p += 1;
            } else {
                *p += 1;
                more = false;
            }
        }
        Jv::Arr(items)
    } else if c == 34 {
        *p += 1;
        let mut h = 0i64;
        while t[*p] != 34 {
            h = (h * 31 + t[*p]) & 2147483647;
            *p += 1;
        }
        *p += 1;
        Jv::Str(h)
    } else if c == 45 || (48..=57).contains(&c) {
        let mut neg = false;
        if c == 45 {
            neg = true;
            *p += 1;
        }
        let mut m = 0i64;
        while t[*p] >= 48 && t[*p] <= 57 {
            m = m * 10 + (t[*p] - 48);
            *p += 1;
        }
        let mut scale = 1.0f64;
        if t[*p] == 46 {
            *p += 1;
            while t[*p] >= 48 && t[*p] <= 57 {
                m = m * 10 + (t[*p] - 48);
                scale *= 10.0;
                *p += 1;
            }
        }
        let f = m as f64 / scale;
        Jv::Num(if neg { 0.0 - f } else { f })
    } else {
        while *p < t.len() && t[*p] >= 97 && t[*p] <= 122 {
            *p += 1;
        }
        Jv::Lit
    }
}

fn put(out: &mut Vec<i64>, frag: &str) {
    for b in frag.bytes() {
        out.push(b as i64);
    }
}

fn digits(out: &mut Vec<i64>, v: i64, width: u32) {
    let mut d = 1i64;
    for _ in 1..width {
        d *= 10;
    }
    while d > 0 {
        out.push(48 + (v / d) % 10);
        d /= 10;
    }
}

fn next(s: i64) -> i64 {
    (s * 1103515245 + 12345) & 2147483647
}

fn show(v: f64) -> String {
    let mut s = format!("{:.6}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

fn main() {
    let records = 20000;
    let mut text: Vec<i64> = Vec::new();
    let mut s = 1i64;
    put(&mut text, "{\n  \"coordinates\": [\n");
    for i in 0..records {
        if i > 0 {
            put(&mut text, ",\n");
        }
        put(&mut text, "    {\n      \"x\": -0.");
        s = next(s);
        digits(&mut text, (s >> 8) % 1000000, 6);
        put(&mut text, ",\n      \"y\": ");
        s = next(s);
        let b = (s >> 8) % 1000000000;
        digits(&mut text, b / 1000000, 1);
        text.push(46);
        digits(&mut text, b % 1000000, 6);
        put(&mut text, ",\n      \"z\": 0.");
        s = next(s);
        digits(&mut text, (s >> 8) % 1000000, 6);
        put(&mut text, ",\n      \"name\": \"");
        for _ in 0..6 {
            s = next(s);
            text.push(97 + (s >> 8) % 26);
        }
        text.push(32);
        s = next(s);
        digits(&mut text, (s >> 8) % 10000, 4);
        put(&mut text, "\",\n      \"opts\": {\n        \"1\": [\n          1,\n          true\n        ]\n      }\n    }");
    }
    put(&mut text, "\n  ],\n  \"info\": \"some info\"\n}\n");

    let mut p = 0;
    let root = parse_value(&text, &mut p);
    let (mut sx, mut sy, mut sz) = (0.0f64, 0.0f64, 0.0f64);
    let mut n = 0i64;
    if let Jv::Dict(keys, vals) = &root {
        for i in 0..keys.len() {
            if keys[i] != 1871919611 {
                continue;
            }
            if let Jv::Arr(items) = &vals[i] {
                for item in items {
                    if let Jv::Dict(k, v) = item {
                        n += 1;
                        for j in 0..k.len() {
                            if let Jv::Num(f) = &v[j] {
                                if k[j] == 120 {
                                    sx += f;
                                } else if k[j] == 121 {
                                    sy += f;
                                } else if k[j] == 122 {
                                    sz += f;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let len = n as f64;
    println!("{} {} {} {}", n, show(sx / len), show(sy / len), show(sz / len));
}
