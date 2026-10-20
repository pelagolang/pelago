//! What the language server knows about a program: the checked package and the queries on it
//! (diagnostics, hover, definitions, references, symbols, semantic tokens).
//!
//! Positions in the protocol are a line and a column counted in UTF-16 units; the compiler uses
//! byte offsets. [`LineMap`] converts between them.

use crate::highlight::{classify, Class};
use crate::lexer::tokenize;
use crate::package::{Diagnostic, ModuleId, Program};
use crate::resolve::{resolve, DefId, DefKind, Resolved};
use crate::span::Span;
use crate::token::{Punct, TokenKind};
use crate::typecheck::{check, Typed};
use crate::types::TyKind;
use std::collections::HashMap;

/// One source file of the package being analysed.
pub struct SourceFile {
    pub uri: String,
    /// The module path inside the package (`geometry.shapes`).
    pub module: String,
    pub text: String,
}

pub struct Analysis {
    pub program: Program,
    pub resolved: Resolved,
    pub typed: Typed,
    pub diags: Vec<Diagnostic>,
    /// The module of each source file, with its uri.
    pub modules: Vec<(String, ModuleId)>,
}

/// Checks a package made of `files` (and the core library).
pub fn analyze(name: &str, files: &[SourceFile]) -> Analysis {
    let mut program = Program::with_core();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|f| (f.module.as_str(), f.text.as_str()))
        .collect();
    let pid = program.add_package(name, &refs);
    let resolved = resolve(&program);
    let typed = check(&program, &resolved);
    let mut diags = typed.diagnostics.clone();
    diags.extend(crate::values::check(&program, &resolved, &typed));
    diags.sort_by_key(|d| (d.module, d.span.start, d.span.end));
    diags.dedup();
    let modules = files
        .iter()
        .filter_map(|f| {
            program
                .modules
                .iter()
                .find(|m| m.package == pid && !m.synthetic && m.path == f.module)
                .map(|m| (f.uri.clone(), m.id))
        })
        .collect();
    Analysis {
        program,
        resolved,
        typed,
        diags,
        modules,
    }
}

// ---- Positions ---------------------------------------------------------------------------------

pub struct LineMap<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineMap<'a> {
    pub fn new(text: &'a str) -> LineMap<'a> {
        let mut starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        LineMap { text, starts }
    }

    /// Line and UTF-16 column of a byte offset.
    pub fn position(&self, off: usize) -> (u32, u32) {
        let off = off.min(self.text.len());
        let line = self.starts.partition_point(|s| *s <= off) - 1;
        let mut start = self.starts[line];
        // Never inside a character.
        let mut end = off;
        while end > start && !self.text.is_char_boundary(end) {
            end -= 1;
        }
        if start > end {
            start = end;
        }
        let col: usize = self.text[start..end].chars().map(char::len_utf16).sum();
        (line as u32, col as u32)
    }

    /// The byte offset of a line and UTF-16 column (clamped to the line).
    pub fn offset(&self, line: u32, col: u32) -> usize {
        let Some(&start) = self.starts.get(line as usize) else {
            return self.text.len();
        };
        let end = self
            .starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.text.len());
        let mut units = 0u32;
        for (i, c) in self.text[start..end].char_indices() {
            if units >= col || c == '\n' {
                return start + i;
            }
            units += c.len_utf16() as u32;
        }
        end
    }
}

// ---- Uris ------------------------------------------------------------------------------------------

pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    uri_to_path_str(uri, cfg!(windows)).map(std::path::PathBuf::from)
}

pub fn path_to_uri(path: &std::path::Path) -> String {
    path_str_to_uri(&path.to_string_lossy(), cfg!(windows))
}

/// `file:///C:/Users/me/x.pel` (or `file:///c%3A/...`) to `C:\Users\me\x.pel` when `windows`;
/// otherwise the plain decoded path.
fn uri_to_path_str(uri: &str, windows: bool) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(h) = h {
                out.push(h);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut p = String::from_utf8(out).ok()?;
    if windows {
        // `/C:/dir` has a drive letter after the first slash.
        let b = p.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            p.remove(0);
        }
        p = p.replace('/', "\\");
        if p.as_bytes().get(1) == Some(&b':') {
            // A drive letter is spelled in capitals, whatever the editor sent.
            let up = p[..1].to_ascii_uppercase();
            p.replace_range(..1, &up);
        }
    }
    Some(p)
}

fn path_str_to_uri(path: &str, windows: bool) -> String {
    let mut p = if windows {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    if windows && p.as_bytes().get(1) == Some(&b':') {
        p.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            b':' if windows => out.push(':'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---- Queries ---------------------------------------------------------------------------------------

/// A symbol of the outline.
pub struct Symbol {
    pub name: String,
    /// The LSP `SymbolKind` number.
    pub kind: u32,
    pub span: Span,
    pub children: Vec<Symbol>,
}

/// The token types the server reports, in legend order.
pub const TOKEN_TYPES: &[&str] = &[
    "keyword",
    "type",
    "function",
    "method",
    "variable",
    "parameter",
    "property",
    "enumMember",
    "number",
    "string",
    "comment",
    "operator",
    "namespace",
    "decorator",
    "typeParameter",
    "struct",
    "enum",
    "interface",
];
pub const TOKEN_MODIFIERS: &[&str] =
    &["declaration", "documentation", "defaultLibrary", "readonly"];

fn tt(name: &str) -> u32 {
    TOKEN_TYPES.iter().position(|t| *t == name).unwrap_or(0) as u32
}

/// A signature label, the byte ranges of its parameters in the label, and the active parameter.
pub type Signature = (String, Vec<(usize, usize)>, usize);

impl Analysis {
    fn text(&self, m: ModuleId) -> &str {
        &self.program.module(m).src
    }

    /// The identifier token that contains the byte offset (or ends right at it).
    pub fn ident_at(&self, m: ModuleId, off: usize) -> Option<Span> {
        let lex = tokenize(self.text(m));
        lex.tokens
            .iter()
            .find(|t| {
                t.kind == TokenKind::Ident
                    && (t.span.start as usize) <= off
                    && off <= t.span.end as usize
            })
            .map(|t| t.span)
    }

    fn def_of_name(&self, m: ModuleId, start: u32) -> Option<DefId> {
        self.resolved
            .uses
            .get(&(m, start))
            .or_else(|| self.resolved.decls.get(&(m, start)))
            .copied()
    }

    pub fn definition(&self, m: ModuleId, off: usize) -> Option<(ModuleId, Span)> {
        let id = self.ident_at(m, off)?;
        let def = self.resolved.def(self.def_of_name(m, id.start)?);
        Some((def.file?, def.span))
    }

    pub fn references(&self, m: ModuleId, off: usize, with_decl: bool) -> Vec<(ModuleId, Span)> {
        let Some(id) = self.ident_at(m, off) else {
            return Vec::new();
        };
        let Some(def) = self.def_of_name(m, id.start) else {
            return Vec::new();
        };
        let len = id.end - id.start;
        let mut out: Vec<(ModuleId, Span)> = self
            .resolved
            .uses
            .iter()
            .filter(|(_, d)| **d == def)
            .map(|((f, s), _)| (*f, Span::new(*s, *s + len)))
            .collect();
        if with_decl {
            let d = self.resolved.def(def);
            if let Some(f) = d.file {
                out.push((f, d.span));
            }
        }
        out.sort_by_key(|(f, s)| (*f, s.start));
        out.dedup();
        out
    }

    /// The name under the cursor if it can be renamed: it must be declared in a file of the package
    /// (not the library, not a built-in).
    pub fn prepare_rename(&self, m: ModuleId, off: usize) -> Option<(String, Span)> {
        let id = self.ident_at(m, off)?;
        let def = self.resolved.def(self.def_of_name(m, id.start)?);
        let file = def.file?;
        if !self.modules.iter().any(|(_, x)| *x == file) || def.kind == DefKind::Module {
            return None;
        }
        Some((def.name.clone(), id))
    }

    /// Every place that has to change to rename the name at `off`; an error message when the new
    /// name is not a plain identifier or the name cannot be renamed.
    pub fn rename(
        &self,
        m: ModuleId,
        off: usize,
        new: &str,
    ) -> Result<Vec<(ModuleId, Span)>, String> {
        let mut chars = new.chars();
        let ident = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_')
            && new != "_";
        if !ident || crate::keyword::Keyword::from_ident(new).is_some() {
            return Err(format!("`{new}` is not a valid name"));
        }
        if self.prepare_rename(m, off).is_none() {
            return Err("this name cannot be renamed".to_string());
        }
        Ok(self.references(m, off, true))
    }

    /// Signature help for the call around `off`: the label, the byte ranges of the parameters
    /// inside the label, and the index of the parameter being written.
    pub fn signature_help(&self, m: ModuleId, off: usize) -> Option<Signature> {
        let lex = tokenize(self.text(m));
        let toks: Vec<_> = lex
            .tokens
            .iter()
            .filter(|t| (t.span.end as usize) <= off)
            .collect();
        // Walk back to the `(` that is still open, counting the commas at its own depth.
        let mut depth = 0i32;
        let mut commas = 0usize;
        let mut open = None;
        for (i, t) in toks.iter().enumerate().rev() {
            match t.kind {
                TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => depth += 1,
                TokenKind::Punct(Punct::LBracket | Punct::LBrace) => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                TokenKind::Punct(Punct::LParen) => {
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Punct(Punct::Comma) if depth == 0 => commas += 1,
                _ => {}
            }
        }
        let callee = toks.get(open?.checked_sub(1)?)?;
        if callee.kind != TokenKind::Ident {
            return None;
        }
        let def = self.def_of_name(m, callee.span.start)?;
        if !matches!(
            self.resolved.def(def).kind,
            DefKind::Fn | DefKind::BuiltinFn
        ) {
            return None;
        }
        let label = self.describe(def);
        let open_paren = label.find('(')?;
        // The parameters are the comma separated pieces of the top level inside the parentheses.
        let mut ranges = Vec::new();
        let (mut d, mut start) = (0i32, open_paren + 1);
        for (i, c) in label.char_indices().skip_while(|(i, _)| *i <= open_paren) {
            match c {
                '(' | '[' | '{' | '<' => d += 1,
                ')' if d == 0 => {
                    if i > start {
                        ranges.push((start, i));
                    }
                    break;
                }
                ')' | ']' | '}' | '>' => d -= 1,
                ',' if d == 0 => {
                    ranges.push((start, i));
                    start = i + 1;
                    while label[start..].starts_with(' ') {
                        start += 1;
                    }
                }
                _ => {}
            }
        }
        let info_has_self = self.typed.fn_infos.get(&def).is_some_and(|i| i.has_self);
        // A method called with a dot does not pass `self`: skip that piece of the label.
        let called_on_value = open
            .and_then(|o| o.checked_sub(2))
            .and_then(|k| toks.get(k))
            .is_some_and(|t| t.kind == TokenKind::Punct(Punct::Dot));
        let skip = usize::from(info_has_self && called_on_value);
        let ranges: Vec<_> = ranges.into_iter().skip(skip).collect();
        let active = commas.min(ranges.len().saturating_sub(1));
        Some((label, ranges, active))
    }

    /// The text shown on hover and the range it applies to.
    pub fn hover(&self, m: ModuleId, off: usize) -> Option<(String, Span)> {
        if let Some(id) = self.ident_at(m, off) {
            if let Some(def) = self.def_of_name(m, id.start) {
                return Some((self.describe(def), id));
            }
        }
        // No name here: the type of the smallest expression around the offset.
        let mut best: Option<(u32, u32, crate::types::Ty)> = None;
        for ((f, s, e), t) in &self.typed.tys {
            if *f == m
                && (*s as usize) <= off
                && off <= *e as usize
                && !self.typed.types.has_vars(*t)
            {
                let better = best.is_none_or(|(bs, be, _)| e - s < be - bs);
                if better {
                    best = Some((*s, *e, *t));
                }
            }
        }
        let (s, e, t) = best?;
        Some((self.typed.show(t), Span::new(s, e)))
    }

    fn describe(&self, def: DefId) -> String {
        let d = self.resolved.def(def);
        let ty = |t| self.typed.show(t);
        match d.kind {
            DefKind::Fn | DefKind::BuiltinFn => {
                let Some(info) = self.typed.fn_infos.get(&def) else {
                    return format!("fn {}", d.name);
                };
                let mut ps: Vec<String> = Vec::new();
                if info.has_self {
                    ps.push(match info.self_mode {
                        Some(m) => format!("{} self", m.as_str()),
                        None => "self".to_string(),
                    });
                }
                for p in &info.params {
                    let mode = p.mode.map_or(String::new(), |m| format!("{} ", m.as_str()));
                    let dots = if p.variadic { "..." } else { "" };
                    ps.push(format!("{mode}{dots}{}: {}", p.name, ty(p.ty)));
                }
                let ret = self.typed.fn_rets.get(&def).copied().unwrap_or(info.ret);
                let ret = if matches!(self.typed.types.kind(ret), TyKind::Void) {
                    String::new()
                } else {
                    format!(" -> {}", ty(ret))
                };
                format!("fn {}({}){ret}", d.name, ps.join(", "))
            }
            DefKind::Struct | DefKind::Obj => {
                let kw = if d.kind == DefKind::Struct {
                    "struct"
                } else {
                    "obj"
                };
                match self.typed.adts.get(&def) {
                    Some(a) if !a.fields.is_empty() => {
                        let fs: Vec<String> = a
                            .fields
                            .iter()
                            .map(|(f, t)| format!("{}: {}", self.resolved.def(*f).name, ty(*t)))
                            .collect();
                        format!("{kw} {} {{ {} }}", d.name, fs.join(", "))
                    }
                    _ => format!("{kw} {}", d.name),
                }
            }
            DefKind::Enum => match self.typed.adts.get(&def) {
                Some(a) if !a.variants.is_empty() => {
                    let vs: Vec<String> = a
                        .variants
                        .iter()
                        .map(|(v, ps)| {
                            let name = &self.resolved.def(*v).name;
                            if ps.is_empty() {
                                name.clone()
                            } else {
                                let ts: Vec<String> = ps.iter().map(|(_, t)| ty(*t)).collect();
                                format!("{name}({})", ts.join(", "))
                            }
                        })
                        .collect();
                    format!("enum {} {{ {} }}", d.name, vs.join(", "))
                }
                _ => format!("enum {}", d.name),
            },
            DefKind::Interface => format!("interface {}", d.name),
            DefKind::TypeAlias => format!("type {}", d.name),
            DefKind::Module => format!("mod {}", d.name),
            DefKind::Variant | DefKind::BuiltinCase => format!("case {}", d.name),
            DefKind::GenericParam => format!("type parameter {}", d.name),
            DefKind::BuiltinType => format!("type {}", d.name),
            DefKind::Const | DefKind::Var | DefKind::Local | DefKind::Param | DefKind::Field => {
                let tag = match d.kind {
                    DefKind::Const => "const ".to_string(),
                    DefKind::Var => "var ".to_string(),
                    DefKind::Param => "(param) ".to_string(),
                    DefKind::Field => "(field) ".to_string(),
                    _ => "(local) ".to_string(),
                };
                match self.typed.local_tys.get(&def) {
                    Some(t) => format!("{tag}{}: {}", d.name, ty(*t)),
                    None => format!("{tag}{}", d.name),
                }
            }
        }
    }

    /// Names that could be written at `off`. Not exact: after a dot, the members of the expression's
    /// type when it is a struct, enum or obj (otherwise every member); elsewhere the keywords, the
    /// declarations of the package and the library, and the locals of the enclosing item.
    pub fn completions(&self, m: ModuleId, off: usize) -> Vec<(String, u32)> {
        let src = self.text(m);
        let off = off.min(src.len());
        // The prefix being typed, and what comes before it.
        let mut start = off;
        while start > 0 {
            let c = src[..start].chars().next_back().unwrap_or(' ');
            if c.is_alphanumeric() || c == '_' {
                start -= c.len_utf8();
            } else {
                break;
            }
        }
        let after_dot = src[..start].ends_with('.') && !src[..start].ends_with("..");
        let kind_num = |k: DefKind| match k {
            DefKind::Fn | DefKind::BuiltinFn => 3,
            DefKind::Struct | DefKind::Obj => 22,
            DefKind::Enum => 13,
            DefKind::Interface => 8,
            DefKind::TypeAlias | DefKind::BuiltinType | DefKind::GenericParam => 7,
            DefKind::Module => 9,
            DefKind::Field => 5,
            DefKind::Variant | DefKind::BuiltinCase => 20,
            DefKind::Const => 21,
            _ => 6,
        };
        let mut out: Vec<(String, u32)> = Vec::new();
        if after_dot {
            let dot = start - 1;
            // The expression just before the dot.
            let owner = self
                .typed
                .tys
                .iter()
                .filter(|((f, _, e), _)| *f == m && *e as usize == dot)
                .min_by_key(|((_, s, e), _)| e - s)
                .and_then(
                    |(_, t)| match self.typed.types.kind(self.typed.types.shallow(*t)) {
                        TyKind::Adt(d, _) => Some(*d),
                        _ => None,
                    },
                );
            for (i, d) in self.resolved.defs.iter().enumerate() {
                let member = d.parent.is_some()
                    && matches!(d.kind, DefKind::Field | DefKind::Fn | DefKind::Variant);
                let wanted = match owner {
                    Some(o) => d.parent == Some(o) || self.impl_method_of(o, DefId(i as u32)),
                    None => true,
                };
                if member && wanted {
                    out.push((d.name.clone(), kind_num(d.kind)));
                }
            }
        } else {
            for k in crate::keyword::Keyword::ALL {
                if k.is_hard() {
                    out.push((k.as_str().to_string(), 14));
                }
            }
            let module = self.program.module(m);
            let here = module
                .file
                .stmts
                .iter()
                .find(|s| (s.span.start as usize) <= off && off <= s.span.end as usize)
                .map(|s| s.span);
            for d in &self.resolved.defs {
                let in_package = d.file.is_some_and(|f| {
                    self.program.module(f).package == module.package
                        || d.vis == crate::ast::Vis::Pub
                });
                let top = d.parent.is_none();
                let local = matches!(d.kind, DefKind::Local | DefKind::Param)
                    && d.file == Some(m)
                    && here
                        .is_some_and(|h| h.start <= d.span.start && (d.span.start as usize) < off);
                let global = top
                    && !matches!(d.kind, DefKind::Local | DefKind::Param | DefKind::Variant)
                    && (d.file.is_none() || in_package);
                if global || local {
                    out.push((d.name.clone(), kind_num(d.kind)));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Is `f` a method written in an `impl` for the type `owner`?
    fn impl_method_of(&self, owner: DefId, f: DefId) -> bool {
        self.typed.impls.iter().any(|i| {
            i.methods.values().any(|m| *m == f)
                && matches!(self.typed.types.kind(self.typed.types.shallow(i.self_ty)), TyKind::Adt(d, _) if *d == owner)
        })
    }

    /// The outline of one module: its declarations, with members nested.
    pub fn symbols(&self, m: ModuleId) -> Vec<Symbol> {
        let mut by_parent: HashMap<Option<DefId>, Vec<DefId>> = HashMap::new();
        for (i, d) in self.resolved.defs.iter().enumerate() {
            if d.file != Some(m) || d.span.end == 0 && d.span.start == 0 {
                continue;
            }
            if !matches!(
                d.kind,
                DefKind::Module
                    | DefKind::Fn
                    | DefKind::Const
                    | DefKind::Var
                    | DefKind::Struct
                    | DefKind::Obj
                    | DefKind::Enum
                    | DefKind::Interface
                    | DefKind::Variant
                    | DefKind::TypeAlias
                    | DefKind::Field
            ) {
                continue;
            }
            // Locals of functions are not part of the outline; their parent is not set.
            by_parent.entry(d.parent).or_default().push(DefId(i as u32));
        }
        for v in by_parent.values_mut() {
            v.sort_by_key(|d| self.resolved.def(*d).span.start);
        }
        self.symbols_under(&by_parent, None)
    }

    fn symbols_under(
        &self,
        by_parent: &HashMap<Option<DefId>, Vec<DefId>>,
        parent: Option<DefId>,
    ) -> Vec<Symbol> {
        let Some(ids) = by_parent.get(&parent) else {
            return Vec::new();
        };
        ids.iter()
            .map(|id| {
                let d = self.resolved.def(*id);
                let kind = match d.kind {
                    DefKind::Module => 2,
                    DefKind::Fn if d.parent.is_some() => 6,
                    DefKind::Fn => 12,
                    DefKind::Const => 14,
                    DefKind::Var => 13,
                    DefKind::Struct => 23,
                    DefKind::Obj => 5,
                    DefKind::Enum => 10,
                    DefKind::Interface => 11,
                    DefKind::Variant => 22,
                    DefKind::Field => 8,
                    _ => 26,
                };
                Symbol {
                    name: d.name.clone(),
                    kind,
                    span: d.span,
                    children: self.symbols_under(by_parent, Some(*id)),
                }
            })
            .collect()
    }

    /// The semantic tokens of a module in the protocol's relative encoding.
    pub fn semantic_tokens(&self, m: ModuleId) -> Vec<u32> {
        let src = self.text(m);
        let map = LineMap::new(src);
        let mut out: Vec<u32> = Vec::new();
        let (mut last_line, mut last_col) = (0u32, 0u32);
        for p in classify(src) {
            let named = self.def_of_name(m, p.start as u32);
            let is_decl = self.resolved.decls.contains_key(&(m, p.start as u32));
            let (ty, mut mods) = match self.token_kind(p.class, named) {
                Some(x) => x,
                None => continue,
            };
            if is_decl {
                mods |= 1;
            }
            // A token cannot span lines: cut it at every newline.
            let mut start = p.start;
            for line in src[p.start..p.end].split_inclusive('\n') {
                let text = line.trim_end_matches(['\n', '\r']);
                if !text.is_empty() {
                    let (l, c) = map.position(start);
                    let len: u32 = text.chars().map(|c| c.len_utf16() as u32).sum();
                    let dl = l - last_line;
                    let dc = if dl == 0 { c - last_col } else { c };
                    out.extend([dl, dc, len, ty, mods]);
                    last_line = l;
                    last_col = c;
                }
                start += line.len();
            }
        }
        out
    }

    /// The token type and modifier bits for a piece, or `None` when it is not coloured.
    fn token_kind(&self, class: Class, def: Option<DefId>) -> Option<(u32, u32)> {
        if let Some(def) = def {
            let d = self.resolved.def(def);
            let (name, mods) = match d.kind {
                DefKind::Module => ("namespace", 0),
                DefKind::Fn if d.parent.is_some() => ("method", 0),
                DefKind::Fn => ("function", 0),
                DefKind::BuiltinFn => ("function", 4),
                DefKind::Const => ("variable", 8),
                DefKind::Var | DefKind::Local => ("variable", 0),
                DefKind::Param => ("parameter", 0),
                DefKind::Field => ("property", 0),
                DefKind::Variant => ("enumMember", 0),
                DefKind::BuiltinCase => ("enumMember", 4),
                DefKind::Struct | DefKind::Obj => ("struct", 0),
                DefKind::Enum => ("enum", 0),
                DefKind::Interface => ("interface", 0),
                DefKind::TypeAlias => ("type", 0),
                DefKind::GenericParam => ("typeParameter", 0),
                DefKind::BuiltinType => ("type", 4),
            };
            if matches!(class, Class::Plain | Class::Type | Class::Function) {
                return Some((tt(name), mods));
            }
        }
        let name = match class {
            Class::Plain | Class::Error => return None,
            Class::Control
            | Class::Declaration
            | Class::Binding
            | Class::Mode
            | Class::Constant
            | Class::SelfName
            | Class::OtherKeyword
            | Class::SoftKeyword => "keyword",
            Class::Type => "type",
            Class::Function => "function",
            Class::Number | Class::Char => "number",
            Class::Str | Class::Escape | Class::Interpolation => "string",
            Class::Comment | Class::DocComment => "comment",
            Class::Attribute => "decorator",
            Class::Operator | Class::Mark | Class::Arrow | Class::Optional => "operator",
        };
        let mods = if class == Class::DocComment { 2 } else { 0 };
        Some((tt(name), mods))
    }
}

#[cfg(test)]
mod uri_tests {
    use super::*;

    #[test]
    fn windows_paths_become_file_uris_and_back() {
        let uri = path_str_to_uri("C:\\Users\\me\\my dir\\a.pel", true);
        assert_eq!(uri, "file:///C:/Users/me/my%20dir/a.pel");
        assert_eq!(
            uri_to_path_str(&uri, true).unwrap(),
            "C:\\Users\\me\\my dir\\a.pel"
        );
        // What VS Code sends: a lower-case drive letter and an escaped colon.
        assert_eq!(
            uri_to_path_str("file:///c%3A/Users/me/a.pel", true).unwrap(),
            "C:\\Users\\me\\a.pel"
        );
    }

    #[test]
    fn unix_paths_are_unchanged() {
        let uri = path_str_to_uri("/home/me/a b.pel", false);
        assert_eq!(uri, "file:///home/me/a%20b.pel");
        assert_eq!(uri_to_path_str(&uri, false).unwrap(), "/home/me/a b.pel");
    }
}
