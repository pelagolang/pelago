//! `@derive(fromJson)`: the decoder of a type, written as source text.
//!
//! The derive appends ordinary Pelago code to the module before it is checked (the spans of the
//! generated items point into the appended text, so nothing else in the compiler has to know about
//! it). The code is what you would write by hand with the helpers of `std.json`
//! (`docs/design/json.md`): a struct is an object read by field name, an enum without payloads is
//! the text of its case, an enum with payloads is an object with one member named after the case.
//! A type needs `import std.json` in the module that derives it.

use crate::ast::*;
use crate::span::Span;
use std::fmt::Write;

/// What the derive produced for a module: the text to append and what it could not do.
#[derive(Default)]
pub struct Expansion {
    pub text: String,
    pub errors: Vec<(Span, String)>,
}

const ERR: &str = "json.json_error";

/// The whole number types: name, smallest, largest, and the method that narrows an `int` to it.
const WHOLE: [(&str, &str, &str, &str); 6] = [
    ("int8", "-128", "127", "wrapToInt8"),
    ("int16", "-32768", "32767", "wrapToInt16"),
    ("int32", "-2147483648", "2147483647", "wrapToInt32"),
    ("uint8", "0", "255", "wrapToByte"),
    ("uint16", "0", "65535", "wrapToUint16"),
    ("uint32", "0", "4294967295", "wrapToUint32"),
];

/// Writes the decoders of every top-level type that derives `fromJson`.
pub fn expand(src: &str, file: &File) -> Expansion {
    let mut g = Gen {
        src,
        out: String::new(),
        errors: Vec::new(),
    };
    for stmt in &file.stmts {
        let StmtKind::Item(item) = &stmt.kind else {
            continue;
        };
        for (word, to) in [("fromJson", false), ("toJson", true)] {
            if !derives(src, &item.attrs, word) {
                continue;
            }
            match &item.kind {
                ItemKind::Struct(d) if to => g.encode_structure(d, item.span),
                ItemKind::Enum(d) if to => g.encode_enumeration(d, item.span),
                ItemKind::Struct(d) => g.structure(d, item.span),
                ItemKind::Enum(d) => g.enumeration(d, item.span),
                _ => g.errors.push((
                    item.span,
                    format!("`{word}` can be derived for a struct or an enum"),
                )),
            }
        }
    }
    Expansion {
        text: g.out,
        errors: g.errors,
    }
}

fn derives(src: &str, attrs: &[Attr], word: &str) -> bool {
    attrs.iter().any(|a| {
        a.name.text(src) == "derive"
            && a.args
                .iter()
                .any(|e| src[e.span.start as usize..e.span.end as usize].trim() == word)
    })
}

struct Gen<'a> {
    src: &'a str,
    out: String,
    errors: Vec<(Span, String)>,
}

/// One field of an object being read.
struct Slot {
    /// The key in the document and the field's name in the struct (or the label in a variant).
    name: String,
    ty: Type,
    default: Option<String>,
}

impl<'a> Gen<'a> {
    fn text(&self, s: Span) -> &'a str {
        &self.src[s.start as usize..s.end as usize]
    }

    fn fail(&mut self, at: Span, message: &str) {
        self.errors.push((at, message.to_string()));
    }

    /// The decoder for a type: the name of a function taking `(json.value, inout [int])`. Leaf and
    /// container types get a small function of their own named `base`; a type of the program is
    /// decoded by its own `fromJson`.
    fn decoder(&mut self, ty: &Type, base: &str) -> Option<String> {
        let ret = self.text(ty.span).to_string();
        match &ty.kind {
            TypeKind::Path { path, args } if args.is_empty() && path.segments.len() == 1 => {
                let name = path.segments[0].text(self.src);
                let body = match name {
                    "int" => "v.toInt(at)".to_string(),
                    "float" => "v.toFloat(at)".to_string(),
                    "float16" => "v.toFloat16(at)".to_string(),
                    "float128" => "v.toFloat128(at)".to_string(),
                    "float256" => "v.toFloat256(at)".to_string(),
                    "int128" => "v.toInt128(at)".to_string(),
                    "uint128" => "v.toUint128(at)".to_string(),
                    "int256" => "v.toInt256(at)".to_string(),
                    "uint256" => "v.toUint256(at)".to_string(),
                    "bool" => "v.toBool(at)".to_string(),
                    "string" => "v.toText(at)".to_string(),
                    _ => {
                        if let Some((_, lo, hi, wrap)) = WHOLE.iter().find(|w| w.0 == name) {
                            format!(
                                "{{\n        const n = v.toRanged({lo}, {hi}, \"{name}\", at)?\n        ok(n.{wrap}())\n    }}"
                            )
                        } else {
                            return Some(format!("{name}FromJson"));
                        }
                    }
                };
                let sep = if body.starts_with('{') { " " } else { " = " };
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR}{sep}{body}\n"
                );
                Some(base.to_string())
            }
            TypeKind::Path { path, .. } if path.segments.len() > 1 => {
                // `shapes.circle`: a type of another module, which has its own `circleFromJson`.
                let last = path.segments.len() - 1;
                let module =
                    &self.src[path.span.start as usize..path.segments[last].span.start as usize];
                Some(format!(
                    "{module}{}FromJson",
                    path.segments[last].text(self.src)
                ))
            }
            TypeKind::List(elem) => {
                let inner = self.decoder(elem, &format!("{base}_item"))?;
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR} {{
    var out: {ret} = []
    switch v {{
        case .items(values):
            for i in 0..values.len() {{
                at.push!(i)
                out.push!({inner}(values[i], inout at)?)
                at.pop!()
            }}
        case _: return err(v.mismatch(\"an array\", at))
    }}
    ok(out)
}}\n"
                );
                Some(base.to_string())
            }
            // `[T; N]`: exactly `N` elements.
            TypeKind::Array(elem, n) => {
                let inner = self.decoder(elem, &format!("{base}_item"))?;
                let n = self.text(n.span).trim().to_string();
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR} {{
    switch v {{
        case .items(values):
            if values.len() != {n} {{ return err(v.wrongLength({n}, at)) }}
            var out: [{elem_text}] = []
            for i in 0..values.len() {{
                at.push!(i)
                out.push!({inner}(values[i], inout at)?)
                at.pop!()
            }}
            const r: {ret} = out.asArray()
            ok(r)
        case _: return err(v.mismatch(\"an array\", at))
    }}
}}\n",
                    elem_text = self.text(elem.span)
                );
                Some(base.to_string())
            }
            // `(A, B; N)` and `braid<A, B; N>`: one flat array, slot `i` read as the type of slot `i mod k`.
            TypeKind::FixedTuple(ts, n, _) => {
                let n = self.text(n.span).trim().to_string();
                let Ok(count) = n.replace('_', "").parse::<usize>() else {
                    self.fail(
                        ty.span,
                        "`fromJson` needs the length of a braid to be a plain number",
                    );
                    return None;
                };
                let mut decs = Vec::new();
                for (i, t) in ts.iter().enumerate() {
                    decs.push(self.decoder(t, &format!("{base}_slot{i}"))?);
                }
                let mut body = String::new();
                for i in 0..count {
                    let _ = write!(
                        body,
                        "            at.push!({i})\n            const s{i} = {}(values[{i}], inout at)?\n            at.pop!()\n",
                        decs[i % decs.len()]
                    );
                }
                let slots: Vec<String> = (0..count).map(|i| format!("s{i}")).collect();
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR} {{
    switch v {{
        case .items(values):
            if values.len() != {n} {{ return err(v.wrongLength({n}, at)) }}
{body}            const r: {ret} = ({})
            ok(r)
        case _: return err(v.mismatch(\"an array\", at))
    }}
}}\n",
                    slots.join(", ")
                );
                Some(base.to_string())
            }
            // `(A, B)`: a two-element array.
            TypeKind::Tuple(ts) if !ts.is_empty() => {
                let mut decs = Vec::new();
                for (i, t) in ts.iter().enumerate() {
                    decs.push(self.decoder(t, &format!("{base}_slot{i}"))?);
                }
                let count = ts.len();
                let mut body = String::new();
                for (i, d) in decs.iter().enumerate() {
                    let _ = write!(
                        body,
                        "            at.push!({i})\n            const s{i} = {d}(values[{i}], inout at)?\n            at.pop!()\n"
                    );
                }
                let slots: Vec<String> = (0..count).map(|i| format!("s{i}")).collect();
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR} {{
    switch v {{
        case .items(values):
            if values.len() != {count} {{ return err(v.wrongLength({count}, at)) }}
{body}            ok(({}))
        case _: return err(v.mismatch(\"an array\", at))
    }}
}}\n",
                    slots.join(", ")
                );
                Some(base.to_string())
            }
            TypeKind::Option(inner_ty) => {
                let inner = self.decoder(inner_ty, &format!("{base}_some"))?;
                let _ = write!(
                    self.out,
                    "\nfn {base}(v: json.value, inout at: [int]) -> {ret} ! {ERR} {{
    switch v {{
        case .nothing: ok(none)
        case _:
            const x = {inner}(v, inout at)?
            ok(x)
    }}
}}\n"
                );
                Some(base.to_string())
            }
            _ => {
                self.fail(
                    ty.span,
                    &format!(
                        "`fromJson` cannot read a field of type `{ret}` (numbers, `bool`, `string`, lists, options and types that derive `fromJson` are supported)"
                    ),
                );
                None
            }
        }
    }

    /// The statements that read `slots` out of the object `v` and make the value with `ctor`.
    fn object_body(&mut self, owner: &str, slots: &[Slot], ctor: &str) -> Option<String> {
        let mut b = String::new();
        let mut args = Vec::new();
        for (i, s) in slots.iter().enumerate() {
            let dec = self.decoder(&s.ty, &format!("jsonDecode_{owner}_{}", s.name))?;
            let optional = s.default.is_some() || matches!(s.ty.kind, TypeKind::Option(_));
            if optional {
                let init = match (&s.default, &s.ty.kind) {
                    (Some(d), _) => d.clone(),
                    _ => "none".to_string(),
                };
                let _ = write!(
                    b,
                    "            var f{i}: {} = {init}\n            const i{i} = v.indexOf(\"{}\")\n            if i{i} >= 0 {{\n                at.push!(i{i})\n                f{i} = {dec}(values[i{i}], inout at)?\n                at.pop!()\n            }}\n",
                    self.text(s.ty.span),
                    s.name
                );
            } else {
                let _ = write!(
                    b,
                    "            const i{i} = v.indexOf(\"{}\")\n            if i{i} < 0 {{ return err(v.missing(\"{}\", at)) }}\n            at.push!(i{i})\n            const f{i} = {dec}(values[i{i}], inout at)?\n            at.pop!()\n",
                    s.name, s.name
                );
            }
            args.push(format!("{}: f{i}", s.name));
        }
        let made = ctor.replace("$", &args.join(", "));
        Some(format!(
            "    switch v {{\n        case .fields(_, values):\n{b}            ok({made})\n        case _: err(v.mismatch(\"an object\", at))\n    }}\n"
        ))
    }

    // ---- Writing: `@derive(toJson)` -------------------------------------------------------------------

    /// The encoder for a type: the name of a function taking the value (read-only) and returning a `json.value`.
    /// It mirrors `decoder`: numbers, `bool`, `string`, lists, `[T; N]`, tuples, braids (one flat array), options
    /// (`none` is `null`), and types of the program that derive `toJson`.
    fn encoder(&mut self, ty: &Type, base: &str) -> Option<String> {
        let ret = self.text(ty.span).to_string();
        let head = format!("\nfn {base}(x: {ret}) -> json.value");
        match &ty.kind {
            TypeKind::Path { path, args } if args.is_empty() && path.segments.len() == 1 => {
                let name = path.segments[0].text(self.src);
                let body = match name {
                    "int" => "json.value.integer(x)".to_string(),
                    "float" => "json.value.number(x)".to_string(),
                    // Too wide for a JSON number that other readers keep exact: written as text (digits, or the shortest text that reads back).
                    "int128" | "uint128" | "int256" | "uint256" | "float16" | "float128"
                    | "float256" => "json.value.text(\"#x\")".to_string(),
                    "bool" => "json.value.flag(x)".to_string(),
                    "string" => "json.value.text(copy x)".to_string(),
                    _ if WHOLE.iter().any(|w| w.0 == name) => {
                        "json.value.integer(x to int)".to_string()
                    }
                    _ => return Some(format!("{name}ToJson")),
                };
                let _ = writeln!(self.out, "{head} = {body}");
                Some(base.to_string())
            }
            TypeKind::Path { path, .. } if path.segments.len() > 1 => {
                let last = path.segments.len() - 1;
                let module =
                    &self.src[path.span.start as usize..path.segments[last].span.start as usize];
                Some(format!(
                    "{module}{}ToJson",
                    path.segments[last].text(self.src)
                ))
            }
            TypeKind::List(elem) | TypeKind::Array(elem, _) => {
                let inner = self.encoder(elem, &format!("{base}_item"))?;
                let _ = write!(
                    self.out,
                    "{head} {{\n    var out: [json.value] = []\n    for i in 0..x.len() {{ out.push!({inner}(x[i])) }}\n    json.value.items(out)\n}}\n"
                );
                Some(base.to_string())
            }
            TypeKind::Tuple(ts) if !ts.is_empty() => {
                let slots: Vec<&Type> = ts.iter().collect();
                self.encode_slots(&head, base, &slots, ts.len(), true)
            }
            TypeKind::FixedTuple(ts, n, _) => {
                let n = self.text(n.span).trim().replace('_', "");
                let Ok(count) = n.parse::<usize>() else {
                    self.fail(
                        ty.span,
                        "`toJson` needs the length of a braid to be a plain number",
                    );
                    return None;
                };
                let slots: Vec<&Type> = ts.iter().collect();
                self.encode_slots(&head, base, &slots, count, false)
            }
            TypeKind::Option(inner_ty) => {
                let inner = self.encoder(inner_ty, &format!("{base}_some"))?;
                let _ = write!(
                    self.out,
                    "{head} {{\n    switch x {{\n        case some(v): {inner}(v)\n        case _: json.value.nothing\n    }}\n}}\n"
                );
                Some(base.to_string())
            }
            _ => {
                self.fail(
                    ty.span,
                    &format!(
                        "`toJson` cannot write a field of type `{ret}` (numbers, `bool`, `string`, lists, arrays, tuples, braids, options and types that derive `toJson` are supported)"
                    ),
                );
                None
            }
        }
    }

    /// A tuple or a braid as one flat array: slot `i` is written by the encoder of the type at `i mod k`.
    fn encode_slots(
        &mut self,
        head: &str,
        base: &str,
        types: &[&Type],
        count: usize,
        dotted: bool,
    ) -> Option<String> {
        let mut encs = Vec::new();
        for (i, t) in types.iter().enumerate() {
            encs.push(self.encoder(t, &format!("{base}_slot{i}"))?);
        }
        let mut body = String::new();
        for i in 0..count {
            let slot = if dotted {
                format!("x.{i}")
            } else {
                format!("x[{i}]")
            };
            let _ = writeln!(
                body,
                "    out.push!({}({slot}))",
                encs[i % encs.len().max(1)]
            );
        }
        let _ = write!(
            self.out,
            "{head} {{\n    var out: [json.value] = []\n{body}    json.value.items(out)\n}}\n"
        );
        Some(base.to_string())
    }

    /// The fields of an object, in order: `json.value.fields(keys, values)`.
    fn object_writer(&mut self, owner: &str, slots: &[Slot], access: &str) -> Option<String> {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for s in slots {
            let enc = self.encoder(&s.ty, &format!("jsonEncode_{owner}_{}", s.name))?;
            keys.push(format!("\"{}\"", s.name));
            values.push(format!("{enc}({access}{})", s.name));
        }
        Some(format!(
            "json.value.fields([{}], [{}])",
            keys.join(", "),
            values.join(", ")
        ))
    }

    fn encode_entry(&mut self, name: &str) {
        let _ = write!(
            self.out,
            "
//~ Writes a `{name}` as JSON text.
pub fn {name}Encode(x: {name}) -> string = {name}ToJson(x).encode()
"
        );
    }

    fn encode_structure(&mut self, d: &TypeDecl, span: Span) {
        if !d.generics.params.is_empty() {
            self.fail(span, "`toJson` cannot be derived for a generic type yet");
            return;
        }
        let name = d.name.text(self.src).to_string();
        let slots: Vec<Slot> = d
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Field(f) => Some(Slot {
                    name: f.name.text(self.src).to_string(),
                    ty: f.ty.clone(),
                    default: None,
                }),
                Member::Item(_) => None,
            })
            .collect();
        let mark = self.out.len();
        let Some(body) = self.object_writer(&name, &slots, "x.") else {
            self.out.truncate(mark);
            return;
        };
        let _ = write!(
            self.out,
            "\n//~ Makes the JSON value of a `{name}`: an object with a member per field.\npub fn {name}ToJson(x: {name}) -> json.value = {body}\n"
        );
        self.encode_entry(&name);
    }

    fn encode_enumeration(&mut self, d: &EnumDecl, span: Span) {
        if !d.generics.params.is_empty() {
            self.fail(span, "`toJson` cannot be derived for a generic type yet");
            return;
        }
        let name = d.name.text(self.src).to_string();
        let mut arms = String::new();
        for v in &d.variants {
            let vname = v.name.text(self.src).to_string();
            if v.fields.is_empty() {
                let _ = writeln!(arms, "        case .{vname}: json.value.text(\"{vname}\")");
                continue;
            }
            let named = v.fields.iter().all(|f| f.label.is_some());
            if named {
                let labels: Vec<String> = v
                    .fields
                    .iter()
                    .map(|f| {
                        f.label
                            .as_ref()
                            .map(|l| l.text(self.src))
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect();
                let slots: Vec<Slot> = v
                    .fields
                    .iter()
                    .zip(labels.iter())
                    .map(|(f, l)| Slot {
                        name: l.clone(),
                        ty: f.ty.clone(),
                        default: None,
                    })
                    .collect();
                let Some(obj) = self.object_writer(&format!("{name}_{vname}"), &slots, "") else {
                    continue;
                };
                let _ = writeln!(
                    arms,
                    "        case .{vname}({}): json.value.fields([\"{vname}\"], [{obj}])",
                    labels.join(", ")
                );
            } else if v.fields.len() == 1 {
                let Some(enc) =
                    self.encoder(&v.fields[0].ty, &format!("jsonEncode_{name}_{vname}_value"))
                else {
                    continue;
                };
                let _ = writeln!(
                    arms,
                    "        case .{vname}(x): json.value.fields([\"{vname}\"], [{enc}(x)])"
                );
            } else {
                self.fail(
                    v.span,
                    "`toJson` writes a case with several unnamed values only when they are named; give them labels",
                );
            }
        }
        let _ = write!(
            self.out,
            "\n//~ Makes the JSON value of a `{name}`: the case name as text, or an object with one member.\npub fn {name}ToJson(x: {name}) -> json.value {{\n    switch x {{\n{arms}    }}\n}}\n"
        );
        self.encode_entry(&name);
    }

    fn decode_entry(&mut self, name: &str) {
        let _ = write!(
            self.out,
            "
//~ Reads a `{name}` from JSON text; an error has a path, a line and a column.
pub fn {name}Decode(text: str) -> {name} ! {ERR} {{
    const root = json.document(text)?
    var at: [int] = []
    const r = {name}FromJson(root, inout at)
    if r.isErr() {{
        switch r {{
            case err(e): return err(json.located(text, root, e))
            case ok(_):
        }}
    }}
    return r
}}
"
        );
    }

    fn structure(&mut self, d: &TypeDecl, span: Span) {
        if !d.generics.params.is_empty() {
            self.fail(span, "`fromJson` cannot be derived for a generic type yet");
            return;
        }
        let name = d.name.text(self.src).to_string();
        let slots: Vec<Slot> = d
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Field(f) => Some(Slot {
                    name: f.name.text(self.src).to_string(),
                    ty: f.ty.clone(),
                    default: f.default.as_ref().map(|e| self.text(e.span).to_string()),
                }),
                Member::Item(_) => None,
            })
            .collect();
        let mark = self.out.len();
        let Some(body) = self.object_body(&name, &slots, &format!("{name} {{ $ }}")) else {
            self.out.truncate(mark);
            return;
        };
        let _ = write!(
            self.out,
            "\n//~ Reads a `{name}` from a parsed document; `at` is the place being read.\npub fn {name}FromJson(v: json.value, inout at: [int]) -> {name} ! {ERR} {{\n{body}}}\n"
        );
        self.decode_entry(&name);
    }

    fn enumeration(&mut self, d: &EnumDecl, span: Span) {
        if !d.generics.params.is_empty() {
            self.fail(span, "`fromJson` cannot be derived for a generic type yet");
            return;
        }
        let name = d.name.text(self.src).to_string();
        let mut texts = String::new();
        let mut objects = String::new();
        for v in &d.variants {
            let vname = v.name.text(self.src).to_string();
            if v.fields.is_empty() {
                let _ = writeln!(
                    texts,
                    "            if t == \"{vname}\" {{ return ok({name}.{vname}) }}"
                );
                continue;
            }
            let helper = format!("jsonDecode_{name}_{vname}");
            let named = v.fields.iter().all(|f| f.label.is_some());
            if named {
                let slots: Vec<Slot> = v
                    .fields
                    .iter()
                    .map(|f| Slot {
                        name: f
                            .label
                            .as_ref()
                            .map(|l| l.text(self.src))
                            .unwrap_or("")
                            .to_string(),
                        ty: f.ty.clone(),
                        default: None,
                    })
                    .collect();
                let Some(body) = self.object_body(
                    &format!("{name}_{vname}"),
                    &slots,
                    &format!("{name}.{vname}($)"),
                ) else {
                    continue;
                };
                let _ = write!(
                    self.out,
                    "\nfn {helper}(v: json.value, inout at: [int]) -> {name} ! {ERR} {{\n{body}}}\n"
                );
            } else if v.fields.len() == 1 {
                let Some(dec) = self.decoder(&v.fields[0].ty, &format!("{helper}_value")) else {
                    continue;
                };
                let _ = write!(
                    self.out,
                    "\nfn {helper}(v: json.value, inout at: [int]) -> {name} ! {ERR} {{\n    const x = {dec}(v, inout at)?\n    ok({name}.{vname}(x))\n}}\n"
                );
            } else {
                self.fail(
                    v.span,
                    "`fromJson` reads a case with several unnamed values only when they are named; give them labels",
                );
                continue;
            }
            let _ = write!(
                objects,
                "            if keys[0] == \"{vname}\" {{\n                const r = {helper}(values[0], inout at)?\n                at.pop!()\n                return ok(r)\n            }}\n"
            );
        }
        let _ = write!(
            self.out,
            "\n//~ Reads a `{name}` from a parsed document; `at` is the place being read.
pub fn {name}FromJson(v: json.value, inout at: [int]) -> {name} ! {ERR} {{
    switch v {{
        case .text(t):
{texts}            err(json.failure(\"unknown case \\\"#t\\\"\", at))
        case .fields(keys, values):
            if keys.len() != 1 {{ return err(v.mismatch(\"an object with one member\", at)) }}
            at.push!(0)
{objects}            err(json.failure(\"unknown case \\\"#(keys[0])\\\"\", at))
        case _: err(v.mismatch(\"text or an object\", at))
    }}
}}
"
        );
        self.decode_entry(&name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expansion(src: &str) -> Expansion {
        let (out, _) = crate::parser::parse_source(src);
        expand(src, &out.file)
    }

    #[test]
    fn a_struct_gets_a_decoder_and_an_entry_point() {
        let e = expansion("@derive(fromJson)\nstruct p { x: int, tags: [string], n: uint8? }");
        assert!(e.errors.is_empty());
        assert!(e.text.contains("pub fn pFromJson("));
        assert!(e.text.contains("pub fn pDecode("));
        assert!(e.text.contains("toRanged(0, 255, \"uint8\""));
    }

    #[test]
    fn types_without_the_derive_are_left_alone() {
        assert!(expansion("@derive(eq)\nstruct p { x: int }")
            .text
            .is_empty());
        assert!(expansion("struct p { x: int }").text.is_empty());
    }

    #[test]
    fn a_field_it_cannot_read_is_an_error_at_the_field() {
        let src = "@derive(fromJson)\nstruct p { m: map<string, int> }";
        let e = expansion(src);
        assert_eq!(e.errors.len(), 1);
        let (span, message) = &e.errors[0];
        assert_eq!(
            &src[span.start as usize..span.end as usize],
            "map<string, int>"
        );
        assert!(message.contains("cannot read a field"));
    }

    #[test]
    fn only_structs_and_enums_can_derive_it() {
        let e = expansion("@derive(fromJson)\nfn f() {}");
        assert_eq!(e.errors.len(), 1);
    }
}
