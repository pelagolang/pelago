# Typed JSON decoding, and reading files (draft 1)

Status: draft, written 2026-11-08 at the owner's request ("do the typed JSON decoding and the file and stdin input").
Card: [S-08](../plans/cards/S-08-json-decode.md). Today `std.json` parses into a `json.value` tree and stops there;
`std.io` reads standard input but there is no way to read a file.

## 1. What the owner gets

```
import std.json
import std.io

@derive(fromJson)
struct server { host: string, port: uint16, tags: [string], tls: bool = false, note: string? }

fn main() {
    const text = io.readFile("server.json")?         // or io.readAll() for standard input
    const s = server.decode(text)?                    // one call: parse, then fill a `server`
    print(s.host, s.port)
}
```

A wrong document is an error with a path and a position, never a half-filled value and never a stop:

```
server.json:3:12: $.port: 70000 does not fit in uint16 (0 to 65535)
```

## 2. The decisions, with the trade-offs

**D1. Decoding is ordinary Pelago code over the `json.value` tree, and `@derive(fromJson)` writes that code.**
Pelago has no reflection yet (`functions.md`, `meta`: "reflection later") and interfaces cannot name a function that has no
`self` from a generic parameter (`T.make()` is refused), so `decode<T>(text)` as one generic function is not possible. Two layers instead:

1. A *reader* in `std.json` (`json.reader`): a value plus its path, with one method per shape: `r.int("port")`, `r.text("host")`,
   `r.list("tags")`, `r.field("tls")` (a sub-reader), `r.optional("note")`. You can write a decoder by hand with it today.
2. `@derive(fromJson)` on a `struct` or `enum` writes exactly the hand-written decoder, as source text appended to the module
   before it is parsed, and adds `T.fromJson(reader)` and `T.decode(text)`. What the derive writes is plain Pelago, so it can
   be read, copied and changed. When `meta` reflection arrives the derive can move into the library without changing any user code.
   *Conventional:* a derive that reads field names (serde, Go struct tags). *Pelago's own:* the output is source anyone could have written; no hidden runtime type table.

**D2. Integers stay exact.** `value` gets a case `integer(value: int)`. The parser makes it for a number without fraction or
exponent that fits 64 bits; every other number stays `number(float)`. Before this, `9007199254740993` silently became `9007199254740992`.
Rules when filling a field: an integer type takes only `integer`, and a value outside the type's range is an *error* (not a wrap, and not a
stop: the data is not the program's bug), with the range in the message; `1.0` and `1e3` are not integers (no silent narrowing, as in
`checker.md`); a `float` field takes `number` or `integer` (a plain `1` for a float field is too common to refuse); `uint8` and the other
narrow types check their range like `int`. *Conventional:* strict range checks (serde does the same). *Pelago's:* the same rule as `+`:
an out-of-range number is never silently changed.

**D3. Errors carry a path and, from the text, a position.** `json.error` has `message`, `path` (`$.servers[2].port`), `line` and `column`.
The tree has no offsets (they would cost memory on every successful decode). When a decode fails, `T.decode(text)` finds the position
*afterwards*: it walks the text along the path (the text is known to be valid JSON, so skipping a value reuses the parser) and counts
lines and columns. A success pays nothing; a failure pays one more pass over a document that is already wrong. `json.parse` errors get
`line L, column C` in their message too (same function, offset to line and column). `T.fromJson(value)` on a tree you built has no text,
so its errors have a path and no position (`line` and `column` are 0).

**D4. Struct mapping.** Each field is read by its name. Missing field: the field's default when it has one (`tls: bool = false`),
`none` for an `option` type, otherwise the error `missing field "port"`. Unknown keys are ignored (documents grow); a strict mode
(`@json(strict)`) is a follow-up. Renaming a field (`@json(name: "user_id")`) needs attributes on fields, which the parser does not have
yet: question 2. A field type may be a number type, `bool`, `string`, a list `[T]`, an `option` (`T?`, and `null` gives `none`), a
map with `string` keys, a tuple, or another struct or enum with `fromJson`. A type without it is a compile error at the derive
("`point` needs `@derive(fromJson)` to be a field of `line`"), not a runtime one.

**D5. Enum mapping.** An enum whose cases carry nothing is a string with the case name (`"red"`). A case with a payload is an object with one
key, the case name, holding the payload as an object of its named fields (`{"circle": {"radius": 2.5}}`); a single unnamed payload may be
written directly. *Conventional:* this is serde's "externally tagged" form. Chosen because it needs no reserved key name and no
buffering, and every decoder can tell the case from the first key.

**D6. Ownership.** The input is a `str` that the decoder only reads. The result owns everything in it (strings are copied out of
the text), so it can outlive the document, move between tasks and be freed by the usual drop. A failed decode drops what it had built (the
language does that by itself, because the generated code is ordinary code using `?`). Borrowing `str` fields straight from the text (no copy) would
need the checker to tie the struct's lifetime to the document; not in version 1, the tree already copies once (question 3).

**D7. Speed.** Two passes (text to tree, tree to value) allocate the tree once. A one-pass decoder reading straight from the text is faster
and skips the tree; it is the natural next step once `meta` can write it per type, and it keeps the same error rules. Not a benchmark target today.

## 3. Reading files

Built and specified in [`input.md`](input.md) (2026-11-08): `std.fs` (`fs.readText(path)`, `fs.readBytes`, `fs.writeText`, `fs.open`, ...) and the
`file` type in `std.io`, with `io_error` as the error, `OsKind::Open` and `Close` on every target. This replaces the `io.readFile` sketch that stood here.
`json.decode` from a file is `server.decode(fs.readText(path)?)`, from a pipe `server.decode(io.readAll())`.

## 4. Order of work (small PRs)

1. This document and card S-08.
2. File input: done, see `input.md`.
3. `value.integer`, line and column in `parse` errors, `json.error`, `json.reader`, hand-written decoder test; `std.md` updated.
4. `@derive(fromJson)`: source expansion, errors for unsupported field types, `T.decode`, example in `examples/programs/`.

## 5. Questions for the owner (defaults used until answered)

1. Is the externally tagged enum form (D5) what you want, or a `"type": "circle"` tag field inside the object? Default: external.
2. Field attributes (`@json(name: "user_id")`) need a small parser change; do it in step 4, or leave renaming to hand-written decoders for now? Default: leave out, add later.
3. Should a later step add zero-copy `str` fields (D6)? Default: not in version 1.
4. Strict mode (unknown keys are errors) as an attribute, later? Default: yes, after step 4.
