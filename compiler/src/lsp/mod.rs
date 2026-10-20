//! `pelago lsp`: a language server speaking the Language Server Protocol on standard input and
//! output (card S-06). No external crates: [`json`] reads and writes the messages.
//!
//! What it offers: diagnostics from every compiler stage, hover (the type or signature), go to
//! definition, find references, the document outline, completion (names, and members after a dot;
//! not scope-exact), and semantic tokens (the colours of
//! `highlight.rs`, refined by what the name resolver knows).
//!
//! The server holds the text of the open documents and re-checks the whole package of a document when
//! something changed (the checker has no incremental mode yet). The package is the workspace folder
//! when the document is inside it, otherwise the single file.

pub mod analysis;
pub mod json;
#[cfg(test)]
mod tests;

use analysis::{
    analyze, path_to_uri, uri_to_path, Analysis, LineMap, SourceFile, TOKEN_MODIFIERS, TOKEN_TYPES,
};
use json::{num, obj, text, Json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::rc::Rc;

pub struct Server {
    root: Option<PathBuf>,
    /// Open documents by normalised uri: the uri the client used and the text.
    docs: HashMap<String, (String, String)>,
    cache: HashMap<String, Rc<Analysis>>,
    /// The uris that currently have diagnostics shown.
    published: HashSet<String>,
    shutting_down: bool,
    pub exited: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

fn norm(uri: &str) -> String {
    uri_to_path(uri).map_or_else(|| uri.to_string(), |p| path_to_uri(&p))
}

fn reply(id: &Json, result: Json) -> Json {
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
}

fn failure(id: &Json, code: i64, message: &str) -> Json {
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("id", id.clone()),
        (
            "error",
            obj(vec![("code", num(code as f64)), ("message", text(message))]),
        ),
    ])
}

fn range_json(map: &LineMap, start: usize, end: usize) -> Json {
    let (l1, c1) = map.position(start);
    let (l2, c2) = map.position(end);
    obj(vec![
        (
            "start",
            obj(vec![("line", num(l1)), ("character", num(c1))]),
        ),
        ("end", obj(vec![("line", num(l2)), ("character", num(c2))])),
    ])
}

impl Server {
    pub fn new() -> Server {
        Server {
            root: None,
            docs: HashMap::new(),
            cache: HashMap::new(),
            published: HashSet::new(),
            shutting_down: false,
            exited: false,
        }
    }

    /// Handles one incoming message; gives the messages to send back.
    pub fn handle(&mut self, msg: &Json) -> Vec<Json> {
        let method = msg.get("method").and_then(Json::str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let id = msg.get("id");
        let mut out = Vec::new();
        match (method, id) {
            ("initialize", Some(id)) => {
                self.root = params
                    .get("rootUri")
                    .and_then(Json::str)
                    .or_else(|| {
                        params
                            .get("workspaceFolders")
                            .and_then(Json::arr)
                            .and_then(|f| f.first())
                            .and_then(|f| f.get("uri"))
                            .and_then(Json::str)
                    })
                    .and_then(uri_to_path);
                out.push(reply(id, self.capabilities()));
            }
            ("shutdown", Some(id)) => {
                self.shutting_down = true;
                out.push(reply(id, Json::Null));
            }
            ("exit", _) => self.exited = true,
            ("textDocument/didOpen", _) => {
                let doc = params.get("textDocument");
                let uri = doc.and_then(|d| d.get("uri")).and_then(Json::str);
                let src = doc.and_then(|d| d.get("text")).and_then(Json::str);
                if let (Some(uri), Some(src)) = (uri, src) {
                    self.docs
                        .insert(norm(uri), (uri.to_string(), src.to_string()));
                    self.cache.clear();
                    out.extend(self.diagnostics_for(uri));
                }
            }
            ("textDocument/didChange", _) => {
                let uri = params
                    .get("textDocument")
                    .and_then(|d| d.get("uri"))
                    .and_then(Json::str);
                // Full-text sync: the last change holds the whole text.
                let src = params
                    .get("contentChanges")
                    .and_then(Json::arr)
                    .and_then(|c| c.last())
                    .and_then(|c| c.get("text"))
                    .and_then(Json::str);
                if let (Some(uri), Some(src)) = (uri, src) {
                    self.docs
                        .insert(norm(uri), (uri.to_string(), src.to_string()));
                    self.cache.clear();
                    out.extend(self.diagnostics_for(uri));
                }
            }
            ("textDocument/didClose", _) => {
                if let Some(uri) = params
                    .get("textDocument")
                    .and_then(|d| d.get("uri"))
                    .and_then(Json::str)
                {
                    self.docs.remove(&norm(uri));
                    self.cache.clear();
                    out.extend(self.diagnostics_for(uri));
                }
            }
            (m, Some(id)) if m.starts_with("textDocument/") => {
                let r = self.request(m, &params);
                out.push(match r {
                    Some(v) => reply(id, v),
                    None => reply(id, Json::Null),
                });
            }
            (_, Some(id)) if !method.is_empty() => {
                out.push(failure(id, -32601, "method not found"));
            }
            _ => {}
        }
        out
    }

    fn capabilities(&self) -> Json {
        let strs = |xs: &[&str]| Json::Arr(xs.iter().map(|x| text(*x)).collect());
        obj(vec![
            (
                "capabilities",
                obj(vec![
                    (
                        "textDocumentSync",
                        obj(vec![("openClose", Json::Bool(true)), ("change", num(1))]),
                    ),
                    ("hoverProvider", Json::Bool(true)),
                    ("definitionProvider", Json::Bool(true)),
                    ("referencesProvider", Json::Bool(true)),
                    (
                        "codeActionProvider",
                        obj(vec![("codeActionKinds", strs(&["quickfix"]))]),
                    ),
                    ("documentSymbolProvider", Json::Bool(true)),
                    (
                        "completionProvider",
                        obj(vec![("triggerCharacters", strs(&["."]))]),
                    ),
                    (
                        "semanticTokensProvider",
                        obj(vec![
                            (
                                "legend",
                                obj(vec![
                                    ("tokenTypes", strs(TOKEN_TYPES)),
                                    ("tokenModifiers", strs(TOKEN_MODIFIERS)),
                                ]),
                            ),
                            ("full", Json::Bool(true)),
                        ]),
                    ),
                ]),
            ),
            (
                "serverInfo",
                obj(vec![
                    ("name", text("pelago")),
                    ("version", text(env!("CARGO_PKG_VERSION"))),
                ]),
            ),
        ])
    }

    // ---- Building the analysis ---------------------------------------------------------------

    /// The text of a file: the open document, or the file on disk.
    fn text_of(&self, uri: &str) -> Option<String> {
        if let Some((_, t)) = self.docs.get(&norm(uri)) {
            return Some(t.clone());
        }
        std::fs::read_to_string(uri_to_path(uri)?).ok()
    }

    /// The checked package that holds `uri`.
    fn analysis_for(&mut self, uri: &str) -> Option<Rc<Analysis>> {
        let path = uri_to_path(uri)?;
        let (key, name, files) = match &self.root {
            Some(root) if path.starts_with(root) => {
                let mut found: Vec<(String, String)> = Vec::new();
                let _ = crate::package::collect_files(root, root, &mut found);
                let mut files: Vec<SourceFile> = found
                    .into_iter()
                    .map(|(module, text)| SourceFile {
                        uri: path_to_uri(&root.join(format!("{}.pel", module.replace('.', "/")))),
                        module,
                        text,
                    })
                    .collect();
                // Open documents replace what is on disk (and may be new files).
                for (u, (_, t)) in &self.docs {
                    let Some(p) = uri_to_path(u) else { continue };
                    let Ok(rel) = p.strip_prefix(root) else {
                        continue;
                    };
                    if p.extension().and_then(|e| e.to_str()) != Some("pel") {
                        continue;
                    }
                    let module = rel
                        .with_extension("")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join(".");
                    match files.iter_mut().find(|f| f.module == module) {
                        Some(f) => f.text = t.clone(),
                        None => files.push(SourceFile {
                            uri: u.clone(),
                            module,
                            text: t.clone(),
                        }),
                    }
                }
                files.sort_by(|a, b| a.module.cmp(&b.module));
                let name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("main")
                    .to_string();
                (root.to_string_lossy().into_owned(), name, files)
            }
            _ => {
                let stem = path.file_stem()?.to_str()?.to_string();
                let src = self.text_of(uri)?;
                let files = vec![SourceFile {
                    uri: norm(uri),
                    module: stem.clone(),
                    text: src,
                }];
                (norm(uri), stem, files)
            }
        };
        if let Some(a) = self.cache.get(&key) {
            return Some(a.clone());
        }
        let a = Rc::new(analyze(&name, &files));
        self.cache.insert(key, a.clone());
        Some(a)
    }

    // ---- Diagnostics -------------------------------------------------------------------------------

    /// Sends the diagnostics of the package that holds `uri`, and clears those that went away.
    fn diagnostics_for(&mut self, uri: &str) -> Vec<Json> {
        let mut out = Vec::new();
        let mut now: HashSet<String> = HashSet::new();
        if let Some(a) = self.analysis_for(uri) {
            for (u, m) in &a.modules {
                let src = &a.program.module(*m).src;
                let map = LineMap::new(src);
                let list: Vec<Json> = a
                    .diags
                    .iter()
                    .filter(|d| d.module == *m)
                    .map(|d| {
                        obj(vec![
                            (
                                "range",
                                range_json(&map, d.span.start as usize, d.span.end as usize),
                            ),
                            ("severity", num(if d.warning { 2 } else { 1 })),
                            ("source", text("pelago")),
                            ("message", text(d.message.clone())),
                        ])
                    })
                    .collect();
                // Unused imports are not compile errors, so they only show in the editor.
                let mut list = list;
                list.extend(a.unused_imports(*m).iter().map(|f| {
                    obj(vec![
                        (
                            "range",
                            range_json(&map, f.trigger.start as usize, f.trigger.end as usize),
                        ),
                        ("severity", num(4)),
                        ("tags", Json::Arr(vec![num(1)])),
                        ("source", text("pelago")),
                        ("code", text("unused-import")),
                        ("message", text("unused import")),
                    ])
                }));
                if !list.is_empty() {
                    now.insert(u.clone());
                }
                if !list.is_empty() || self.published.contains(u) {
                    let shown = self.docs.get(u).map_or(u.clone(), |(orig, _)| orig.clone());
                    out.push(publish(&shown, list));
                }
            }
        }
        // The document itself was closed or its package is gone: clear what is left.
        for u in self.published.iter().filter(|u| !now.contains(*u)) {
            if !out.iter().any(|m| {
                m.get("params")
                    .and_then(|p| p.get("uri"))
                    .and_then(Json::str)
                    == Some(u)
            }) {
                out.push(publish(u, Vec::new()));
            }
        }
        self.published = now;
        out
    }

    // ---- Requests --------------------------------------------------------------------------------

    fn request(&mut self, method: &str, params: &Json) -> Option<Json> {
        let uri = params
            .get("textDocument")
            .and_then(|d| d.get("uri"))
            .and_then(Json::str)?
            .to_string();
        let a = self.analysis_for(&uri)?;
        let n = norm(&uri);
        let m = a.modules.iter().find(|(u, _)| *u == n)?.1;
        let src = a.program.module(m).src.clone();
        let map = LineMap::new(&src);
        let off = || {
            let pos = params.get("position")?;
            Some(map.offset(
                pos.get("line")?.int()? as u32,
                pos.get("character")?.int()? as u32,
            ))
        };
        let location = |fm: crate::package::ModuleId, s: crate::span::Span| -> Option<Json> {
            let (u, _) = a.modules.iter().find(|(_, x)| *x == fm)?;
            let shown = self.docs.get(u).map_or(u.clone(), |(orig, _)| orig.clone());
            let text = &a.program.module(fm).src;
            let lm = LineMap::new(text);
            Some(obj(vec![
                ("uri", text_json(&shown)),
                ("range", range_json(&lm, s.start as usize, s.end as usize)),
            ]))
        };
        match method {
            "textDocument/hover" => {
                let (info, span) = a.hover(m, off()?)?;
                Some(obj(vec![
                    (
                        "contents",
                        obj(vec![
                            ("kind", text("markdown")),
                            ("value", text(format!("```pelago\n{info}\n```"))),
                        ]),
                    ),
                    (
                        "range",
                        range_json(&map, span.start as usize, span.end as usize),
                    ),
                ]))
            }
            "textDocument/definition" => {
                let (fm, s) = a.definition(m, off()?)?;
                location(fm, s)
            }
            "textDocument/references" => {
                let with_decl = params
                    .get("context")
                    .and_then(|c| c.get("includeDeclaration"))
                    .is_some_and(|b| *b == Json::Bool(true));
                let refs = a.references(m, off()?, with_decl);
                Some(Json::Arr(
                    refs.into_iter()
                        .filter_map(|(f, s)| location(f, s))
                        .collect(),
                ))
            }
            "textDocument/documentSymbol" => {
                fn conv(map: &LineMap, s: &analysis::Symbol) -> Json {
                    let r = range_json(map, s.span.start as usize, s.span.end as usize);
                    obj(vec![
                        ("name", text(s.name.clone())),
                        ("kind", num(s.kind)),
                        ("range", r.clone()),
                        ("selectionRange", r),
                        (
                            "children",
                            Json::Arr(s.children.iter().map(|c| conv(map, c)).collect()),
                        ),
                    ])
                }
                Some(Json::Arr(
                    a.symbols(m).iter().map(|s| conv(&map, s)).collect(),
                ))
            }
            "textDocument/codeAction" => {
                let r = params.get("range")?;
                let at = |k: &str| {
                    let p = r.get(k)?;
                    Some(map.offset(
                        p.get("line")?.int()? as u32,
                        p.get("character")?.int()? as u32,
                    ) as u32)
                };
                let (from, to) = (at("start")?, at("end")?);
                let mut fixes = a.suggestion_fixes(m);
                fixes.extend(a.unused_imports(m));
                Some(Json::Arr(
                    fixes
                        .into_iter()
                        .filter(|f| f.trigger.start <= to && from <= f.trigger.end)
                        .map(|f| {
                            let edit = obj(vec![
                                (
                                    "range",
                                    range_json(&map, f.edit.start as usize, f.edit.end as usize),
                                ),
                                ("newText", text(f.new_text)),
                            ]);
                            obj(vec![
                                ("title", text(f.title)),
                                ("kind", text("quickfix")),
                                (
                                    "edit",
                                    obj(vec![(
                                        "changes",
                                        obj(vec![(uri.as_str(), Json::Arr(vec![edit]))]),
                                    )]),
                                ),
                            ])
                        })
                        .collect(),
                ))
            }
            "textDocument/completion" => {
                let items = a.completions(m, off()?);
                Some(Json::Arr(
                    items
                        .into_iter()
                        .map(|(label, kind)| obj(vec![("label", text(label)), ("kind", num(kind))]))
                        .collect(),
                ))
            }
            "textDocument/semanticTokens/full" => Some(obj(vec![(
                "data",
                Json::Arr(a.semantic_tokens(m).into_iter().map(num).collect()),
            )])),
            _ => None,
        }
    }
}

fn text_json(s: &str) -> Json {
    text(s)
}

fn publish(uri: &str, diagnostics: Vec<Json>) -> Json {
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("method", text("textDocument/publishDiagnostics")),
        (
            "params",
            obj(vec![
                ("uri", text(uri)),
                ("diagnostics", Json::Arr(diagnostics)),
            ]),
        ),
    ])
}

// ---- The wire ---------------------------------------------------------------------------------------

/// Reads one message body framed by `Content-Length`. `None` at the end of the input.
pub fn read_frame(input: &mut impl BufRead) -> Option<String> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            len = v.trim().parse().ok();
        }
    }
    let len = len?;
    // Do not trust a huge length from a broken peer.
    if len > 256 * 1024 * 1024 {
        return None;
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

pub fn write_frame(out: &mut impl Write, msg: &Json) -> std::io::Result<()> {
    let body = msg.dump();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    out.flush()
}

/// Runs the server on standard input and output; the process exit code.
pub fn run_stdio() -> i32 {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut server = Server::new();
    while let Some(body) = read_frame(&mut input) {
        let Ok(msg) = Json::parse(&body) else {
            continue;
        };
        for reply in server.handle(&msg) {
            if write_frame(&mut output, &reply).is_err() {
                return 1;
            }
        }
        if server.exited {
            return if server.shutting_down { 0 } else { 1 };
        }
    }
    1
}
