use super::format;
use crate::ast_print::print_file;
use crate::parser::parse_and_check;

fn fmt(src: &str) -> String {
    format(src).expect("formats")
}

#[test]
fn indents_by_open_brackets_and_trims() {
    let src = "fn main() {\n  const x = 1   \n  if x == 1 {\n  print(x)\n}\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    const x = 1\n    if x == 1 {\n        print(x)\n    }\n}\n"
    );
}

#[test]
fn commas_blank_lines_and_trailing_comments() {
    let src = "\n\nfn f(a: int,b: int , c: int) {   /// note\n\n\n    g(a,b)\n\n}\n\n\n\nfn g() {}/// end\n";
    assert_eq!(
        fmt(src),
        "fn f(a: int, b: int, c: int) {   /// note\n    g(a, b)\n}\n\nfn g() {} /// end\n"
    );
}

#[test]
fn levels_the_author_added_are_kept() {
    let src = "fn f() {\n    const chain = items\n        .map(~> it)\n        .to_list()\n  switch x {\n        case 1:\n            g()\n  }\n}\n";
    assert_eq!(
        fmt(src),
        "fn f() {\n    const chain = items\n        .map(~> it)\n        .to_list()\n    switch x {\n        case 1:\n            g()\n    }\n}\n"
    );
    let calls = "fn f() {\n    g(\n  1,\n  2,\n)\n}\n";
    assert_eq!(
        fmt(calls),
        "fn f() {\n    g(\n        1,\n        2,\n    )\n}\n"
    );
}

#[test]
fn comments_keep_their_text_and_crlf_is_kept() {
    let src =
        "//~ Docs.\r\n//~* a\n b *~//\r\nfn f() {\r\n/// inside\r\n    ///* c */// f()\r\n}\r\n";
    let out = fmt(src);
    assert!(out.contains("//~* a\r\n b *~//"));
    assert!(out.contains("    /// inside\r\n"));
    assert!(out.contains("    ///* c */// f()"));
    assert!(out.ends_with("}\r\n"));
}

#[test]
fn files_with_errors_are_refused() {
    assert!(format("fn f( {\n").is_err());
    assert!(format("const s = \"abc\n").is_err());
}

#[test]
fn formatting_is_idempotent_and_keeps_the_tree_for_every_example() {
    let mut files = Vec::new();
    for dir in ["../examples", "../examples/programs"] {
        for e in std::fs::read_dir(format!("{}/{dir}", env!("CARGO_MANIFEST_DIR"))).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) == Some("pel") {
                files.push(p);
            }
        }
    }
    assert!(!files.is_empty());
    for p in files {
        let src = std::fs::read_to_string(&p).unwrap();
        let once = match format(&src) {
            Ok(o) => o,
            // A sample that is not meant to parse cleanly (a syntax tour of errors).
            Err(_) => continue,
        };
        assert_eq!(
            format(&once).unwrap(),
            once,
            "not idempotent: {}",
            p.display()
        );
        let tree = |s: &str| print_file(&parse_and_check(s).0.file, s);
        assert_eq!(tree(&src), tree(&once), "tree changed: {}", p.display());
    }
}

#[test]
fn narrow_widths_keep_every_example_valid_and_stable() {
    for dir in ["../examples", "../examples/programs"] {
        for e in std::fs::read_dir(format!("{}/{dir}", env!("CARGO_MANIFEST_DIR"))).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) != Some("pel") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            if format(&src).is_err() {
                continue;
            }
            for width in [60, 40, 20] {
                let config = crate::config::FmtConfig {
                    width,
                    ..Default::default()
                };
                let once = super::format_with(&src, &config)
                    .unwrap_or_else(|e| panic!("{} at width {width}: {e}", p.display()));
                assert_eq!(
                    super::format_with(&once, &config).unwrap(),
                    once,
                    "not idempotent at width {width}: {}",
                    p.display()
                );
            }
        }
    }
}

#[test]
fn indent_width_comes_from_the_config() {
    let config = crate::config::FmtConfig {
        indent: 2,
        ..Default::default()
    };
    let src = "fn main() {\nif true {\nprint(1)\n}\n}\n";
    let out = super::format_with(src, &config).expect("formats");
    assert_eq!(out, "fn main() {\n  if true {\n    print(1)\n  }\n}\n");
    assert_eq!(super::format_with(&out, &config).unwrap(), out);
}

fn narrow(width: usize) -> crate::config::FmtConfig {
    crate::config::FmtConfig {
        width,
        ..Default::default()
    }
}

#[test]
fn a_long_call_breaks_one_argument_per_line_with_a_trailing_comma() {
    let src = "fn main() {\n    const total = add_numbers(first_number, second_number, third_number)\n}\n";
    let out = super::format_with(src, &narrow(40)).expect("formats");
    assert_eq!(
        out,
        "fn main() {\n    const total = add_numbers(\n        first_number,\n        second_number,\n        third_number,\n    )\n}\n"
    );
    assert_eq!(super::format_with(&out, &narrow(40)).unwrap(), out);
}

#[test]
fn a_long_signature_breaks_and_the_body_stays_indented() {
    let src = "fn area(width: int, height: int, depth: int) -> int {\n    return width * height * depth\n}\n";
    let out = super::format_with(src, &narrow(40)).expect("formats");
    assert_eq!(
        out,
        "fn area(\n    width: int,\n    height: int,\n    depth: int,\n) -> int {\n    return width * height * depth\n}\n"
    );
    assert_eq!(super::format_with(&out, &narrow(40)).unwrap(), out);
}

#[test]
fn nested_lists_break_from_the_outside_in() {
    let src = "fn main() {\n    run(alpha(one, two, three), beta(four, five, six))\n}\n";
    let out = super::format_with(src, &narrow(30)).expect("formats");
    assert_eq!(
        out,
        "fn main() {\n    run(\n        alpha(\n            one,\n            two,\n            three,\n        ),\n        beta(four, five, six),\n    )\n}\n"
    );
    assert_eq!(super::format_with(&out, &narrow(30)).unwrap(), out);
}

#[test]
fn lines_that_fit_comments_and_single_arguments_are_left_alone() {
    let fits = "fn main() {\n    run(a, b)\n}\n";
    assert_eq!(super::format_with(fits, &narrow(40)).unwrap(), fits);
    let single = "fn main() {\n    run(a_very_long_single_argument_name_here)\n}\n";
    assert_eq!(super::format_with(single, &narrow(20)).unwrap(), single);
    let comment = "fn main() {\n    run(first_argument, second_argument) /// note\n}\n";
    assert_eq!(super::format_with(comment, &narrow(20)).unwrap(), comment);
}

#[test]
fn import_names_are_sorted_with_the_glob_last() {
    let src = "import { write, read: fetch, dup, ... } = std.io\nimport { b, a } = .util\n";
    assert_eq!(
        fmt(src),
        "import { dup, read: fetch, write, ... } = std.io\nimport { a, b } = .util\n"
    );
    let multi = "import {\n    zeta,\n    alpha,\n} = std.x\n";
    assert_eq!(fmt(multi), "import {\n    alpha,\n    zeta,\n} = std.x\n");
    let commented = "import {\n    b, /// why\n    a,\n} = std.x\n";
    assert_eq!(fmt(commented), commented);
}

#[test]
fn a_long_condition_breaks_after_the_loosest_operator() {
    let src = "fn main() {\n    if first_condition && second_condition || third_condition && fourth_condition {\n        go()\n    }\n}\n";
    let out = super::format_with(src, &narrow(60)).expect("formats");
    assert_eq!(
        out,
        "fn main() {\n    if first_condition && second_condition ||\n        third_condition && fourth_condition {\n        go()\n    }\n}\n"
    );
    assert_eq!(super::format_with(&out, &narrow(60)).unwrap(), out);
}

#[test]
fn a_long_sum_breaks_after_the_plus_but_not_after_a_unary_minus() {
    let src = "fn main() {\n    const total = alpha_value + beta_value + gamma_value\n    const neg = -alpha_value\n}\n";
    let out = super::format_with(src, &narrow(40)).expect("formats");
    assert_eq!(
        out,
        "fn main() {\n    const total = alpha_value +\n        beta_value +\n        gamma_value\n    const neg = -alpha_value\n}\n"
    );
    assert_eq!(super::format_with(&out, &narrow(40)).unwrap(), out);
}
