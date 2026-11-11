#!/usr/bin/env python3
"""Writes regex_backref.pel and regex_backref.out: patterns with backreferences, expected lines from Python's `re`.

Run from this directory: python3 gen_regex_backref.py
"""
import re

CASES = [
    (r"(a|bc)\1", "xbcbcx aa abca"),
    (r"(\w)\1", "hello bookkeeper"),
    (r"(?P<q>['\"]).*?(?P=q)", "say 'hi' and \"yo\" now 'x"),
    (r"(?P<q>['\"]).*?\k<q>", None),  # \k<name> is ours; Python gets (?P=q), see below
    (r"(a)?\1", "b aa"),
    (r"(a*)*\1b", "aaab aaaac"),
    (r"(\d+)-\1", "12-12 3-4 55-5"),
    (r"(\w+) \1", "the the cat sat sat down"),
    (r"\b(\w+)\s+\1\b", "it is is a test test of of"),
    (r"(?i)(a)\1", "aA Aa aa b"),
    (r"(a|b)*\1", "abab abba"),
    (r"(.)(.)\2\1", "abba xyyx abab"),
    (r"(<(\w+)>).*?</\2>", "<b>bold</b> <i>it</i> <u>x</b>"),
    (r"^(.+)\1$", "abcabc abab"),
    (r"(a+)b\1", "aabaa abaaa"),
    (r"(a)(b)?\2", "ab a aab"),
    (r"(a?)\1b", "aab ab b"),
    (r"(\w)(?:\1)+", "aaabbc zzzz"),
    (r"(a{1,2})\1", "aaaa aaa a"),
    (r"(.)\1{2}", "aaa abbbc"),
    (r"([ab])([ab])\2\1", "abba baab abab"),
    (r"(?P<x>a|b)(?P=x)", "aabbab"),
    (r"(abc|ab)\1", "abcabc abab"),
    (r"(.*)\1", "abab"),
    (r"(.*?)\1", "aa"),
]

def py(pattern):
    return pattern.replace(r"\k<q>", "(?P=q)")

def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")

src = [
    "/// Backreferences in `std.regex` (`compileWithBackrefs`, `search`, `searchAll`); the expected lines come from Python's `re`.",
    "import std.regex",
    "",
    "fn spans(hits: [regex.found]) -> string {",
    "    var out = \"\"",
    "    var i = 0",
    "    while i < hits.len() {",
    "        out.pushStr!(\"#(hits[i].start)-#(hits[i].end) \")",
    "        i += 1",
    "    }",
    "    \"#(out.trim())\"",
    "}",
    "",
    "fn t(pattern: str, text: str) -> int ! parse_error {",
    "    const re = regex.compileWithBackrefs(pattern)?",
    "    switch re.searchAll(text, 10000000) {",
    "        case some(hits): print(true, hits.len(), spans(hits))",
    "        case none: print(false, 0, \"\")",
    "    }",
    "    ok(0)",
    "}",
    "",
    "fn g(pattern: str, text: str, budget: int) -> int ! parse_error {",
    "    const re = regex.compileWithBackrefs(pattern)?",
    "    switch re.search(text, budget) {",
    "        case .matched(m): print(\"matched\", m.start, m.end)",
    "        case .nomatch: print(\"nomatch\")",
    "        case .gaveUp: print(\"gaveUp\")",
    "    }",
    "    ok(0)",
    "}",
    "",
    "fn main() {",
]
out = []
for pattern, text in CASES:
    if text is None:
        text = "say 'hi' and \"yo\" now 'x"
    ms = list(re.finditer(py(pattern), text))
    src.append('    t("%s", "%s").or(1)' % (esc(pattern), esc(text)))
    out.append("true %d %s" % (len(ms), " ".join("%d-%d" % m.span() for m in ms)))
# Refused on purpose (the reversed lookahead scan cannot read a capture): see docs/design/regex.md.
src.append('    print(regex.compileWithBackrefs("(x)(?=\\\\1)").isOk(), regex.compile("(a)\\\\1").isOk(), regex.compile("\\\\1(a)").isOk(), regex.compile("(a\\\\1)").isOk())')
out.append("false false false false")
# The budget: a deterministic count of states, never a clock.
src.append('    g("(a*)*\\\\1b", "aaaaaaaaaaaaaaaaaaaac", 50).or(1)')
src.append('    g("(a*)*\\\\1b", "aaaaaaaaaaaaaaaaaaaac", 100000).or(1)')
src.append('    g("(a*)*\\\\1b", "aaaaaaaaaaaaaaaaaaaab", 100000).or(1)')
out += ["gaveUp", "nomatch", "matched 0 21"]
src.append("}")
open("regex_backref.pel", "w").write("\n".join(src) + "\n")
open("regex_backref.out", "w").write("\n".join(out) + "\n")
