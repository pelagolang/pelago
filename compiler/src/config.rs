//! `pelago.yaml`: the settings file for everything about the language that is not source code.
//! Today it has one section, `fmt`; others (the package, the build) will join it later.
//!
//! The file is a strict block-style subset of YAML, read without a library, meant to match the
//! reader in Islet (`islet.yaml`): `# comments`, `key: value` lines and `key:` lines that open a
//! mapping on the lines indented below them. Values are plain text, so `4` is the text `4` until
//! a key asks for a number. There are no anchors, tags, quotes, lists, flow `{}` / `[]`, block
//! scalars, multi-document files or tabs. The first key must be `format: 1`. Unknown sections
//! and keys are errors, so a typo does not silently do nothing.
//!
//! A tool looks for the file from the directory of the file it works on, upwards ([`Config::find`]).
//! A `pelago.toml` (the old name, TOML syntax) is still read when no `pelago.yaml` sits next to
//! it, with a deprecation warning on standard error; that support will be removed.

use std::path::Path;

/// The settings of `pelago fmt` and of formatting in the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmtConfig {
    /// Spaces for one indentation level, 1 to 8.
    pub indent: usize,
    /// The line width the formatter aims at (used when it wraps lines; 20 to 400).
    pub width: usize,
}

impl Default for FmtConfig {
    fn default() -> FmtConfig {
        FmtConfig {
            indent: 4,
            width: 100,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub fmt: FmtConfig,
}

impl Config {
    /// Reads the text of a `pelago.yaml`.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut config = Config::default();
        let mut seen_format = false;
        let mut section: Option<String> = None;
        let mut inner_indent: Option<usize> = None;
        for (i, raw) in text.lines().enumerate() {
            let at = |m: &str| format!("line {}: {m}", i + 1);
            if raw.contains('\t') {
                return Err(at("tabs are not allowed; indent with spaces"));
            }
            let line = strip_yaml_comment(raw).trim_end();
            if line.trim().is_empty() {
                continue;
            }
            if line == "---" || line == "..." {
                return Err(at(
                    "document markers are not supported; one document per file",
                ));
            }
            let indent = line.len() - line.trim_start().len();
            let body = line.trim_start();
            let (key, value) = body
                .split_once(':')
                .filter(|(_, v)| v.is_empty() || v.starts_with(' '))
                .ok_or_else(|| at("expected `key: value` or `key:`"))?;
            let value = value.trim();
            if key.is_empty()
                || key.contains(|c: char| {
                    c.is_whitespace() || "\"'{}[]&*!|>%@`,-".contains(c) && key.starts_with(c)
                })
            {
                return Err(at(&format!("`{key}` is not a plain key")));
            }
            if value.starts_with(|c: char| "\"'{[&*!|>%@`".contains(c)) || value.starts_with("- ") {
                return Err(at(
                    "only plain text values are supported (no quotes, lists, `{}`, anchors or tags)",
                ));
            }
            if indent == 0 {
                section = None;
                inner_indent = None;
                if !seen_format {
                    if key != "format" {
                        return Err(at("the first key must be `format: 1`"));
                    }
                    if value != "1" {
                        return Err(at(&format!(
                            "unsupported `format: {value}` (this pelago reads `format: 1`)"
                        )));
                    }
                    seen_format = true;
                    continue;
                }
                match key {
                    "format" => return Err(at("`format` appears twice")),
                    "fmt" if value.is_empty() => section = Some("fmt".to_string()),
                    "fmt" => return Err(at("`fmt` is a section; put its keys indented below it")),
                    _ => return Err(at(&format!("unknown key `{key}` (known: `format`, `fmt`)"))),
                }
                continue;
            }
            let Some(sec) = section.as_deref() else {
                return Err(at(&format!(
                    "`{key}` is indented but has no section above it"
                )));
            };
            match inner_indent {
                None => inner_indent = Some(indent),
                Some(n) if n != indent => return Err(at("inconsistent indentation")),
                _ => {}
            }
            debug_assert_eq!(sec, "fmt");
            let n = int(value).ok_or_else(|| at(&format!("`{key}` must be a number")))?;
            match key {
                "indent" if (1..=8).contains(&n) => config.fmt.indent = n,
                "indent" => return Err(at("`indent` must be from 1 to 8")),
                "width" if (20..=400).contains(&n) => config.fmt.width = n,
                "width" => return Err(at("`width` must be from 20 to 400")),
                _ => {
                    return Err(at(&format!(
                        "unknown key `{key}` in `fmt` (known: `indent`, `width`)"
                    )))
                }
            }
        }
        Ok(config)
    }

    /// Reads the text of a `pelago.toml` (deprecated).
    pub fn parse_toml(text: &str) -> Result<Config, String> {
        let mut config = Config::default();
        let mut section: Option<String> = None;
        for (i, raw) in text.lines().enumerate() {
            let at = |m: &str| format!("line {}: {m}", i + 1);
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[') {
                let name = name
                    .strip_suffix(']')
                    .ok_or_else(|| at("a section header must end with `]`"))?
                    .trim();
                if name != "fmt" {
                    return Err(at(&format!("unknown section `[{name}]` (known: `[fmt]`)")));
                }
                section = Some(name.to_string());
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| at("expected `key = value`"))?;
            let (key, value) = (key.trim(), value.trim());
            match section.as_deref() {
                None => return Err(at(&format!("`{key}` is outside any section"))),
                Some("fmt") => {
                    let n = int(value).ok_or_else(|| at(&format!("`{key}` must be a number")))?;
                    match key {
                        "indent" if (1..=8).contains(&n) => config.fmt.indent = n,
                        "indent" => return Err(at("`indent` must be from 1 to 8")),
                        "width" if (20..=400).contains(&n) => config.fmt.width = n,
                        "width" => return Err(at("`width` must be from 20 to 400")),
                        _ => {
                            return Err(at(&format!(
                                "unknown key `{key}` in `[fmt]` (known: `indent`, `width`)"
                            )))
                        }
                    }
                }
                Some(_) => unreachable!("only known sections are accepted"),
            }
        }
        Ok(config)
    }

    /// The `pelago.yaml` closest above `path` (a file or a directory), or the defaults when there
    /// is none. An old `pelago.toml` counts when no `pelago.yaml` is in the same directory, and
    /// prints a deprecation warning. An error names the file that could not be read.
    pub fn find(path: &Path) -> Result<Config, String> {
        let start = if path.is_dir() {
            Some(path)
        } else {
            path.parent()
        };
        let mut dir = start.map(|d| {
            if d.as_os_str().is_empty() {
                Path::new(".")
            } else {
                d
            }
        });
        while let Some(d) = dir {
            let file = d.join("pelago.yaml");
            let old = d.join("pelago.toml");
            let read =
                |f: &Path| std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()));
            if file.is_file() {
                return Config::parse(&read(&file)?)
                    .map_err(|e| format!("{}: {e}", file.display()));
            }
            if old.is_file() {
                eprintln!(
                    "warning: {} is deprecated; rename it to pelago.yaml and write it in YAML \
                     (`format: 1`, then `fmt:` with indented `indent: 4` and `width: 100`)",
                    old.display()
                );
                return Config::parse_toml(&read(&old)?)
                    .map_err(|e| format!("{}: {e}", old.display()));
            }
            dir = d.parent();
        }
        Ok(Config::default())
    }
}

/// Cuts a `#` comment: one at the start of the line or after a space.
fn strip_yaml_comment(line: &str) -> &str {
    let mut prev_space = true;
    for (i, c) in line.char_indices() {
        if c == '#' && prev_space {
            return &line[..i];
        }
        prev_space = c == ' ';
    }
    line
}

fn strip_comment(line: &str) -> &str {
    // No value holds a `#` yet (strings are not used), so the first one starts a comment.
    line.split('#').next().unwrap_or("")
}

fn int(value: &str) -> Option<usize> {
    value.replace('_', "").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_values() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::parse("# only a note\n").unwrap(), Config::default());
        assert_eq!(Config::parse("format: 1\n").unwrap(), Config::default());
        let c = Config::parse("# top\nformat: 1\nfmt:\n  indent: 2  # two\n  width: 80\n").unwrap();
        assert_eq!(
            c.fmt,
            FmtConfig {
                indent: 2,
                width: 80
            }
        );
    }

    #[test]
    fn mistakes_are_errors_with_a_line() {
        for (text, want) in [
            (
                "format: 1\nfmt:\n  indnt: 2\n",
                "line 3: unknown key `indnt`",
            ),
            ("format: 1\nformat: 1\n", "line 2: `format` appears twice"),
            ("format: 1\nfromat: 1\n", "line 2: unknown key `fromat`"),
            (
                "fmt:\n  indent: 2\n",
                "line 1: the first key must be `format: 1`",
            ),
            ("format: 2\n", "line 1: unsupported `format: 2`"),
            (
                "format: 1\n  indent: 2\n",
                "line 2: `indent` is indented but has no section",
            ),
            (
                "format: 1\nfmt:\n  indent: 0\n",
                "line 3: `indent` must be from 1 to 8",
            ),
            (
                "format: 1\nfmt:\n  width: wide\n",
                "line 3: `width` must be a number",
            ),
            (
                "format: 1\nfmt:\n\tindent: 2\n",
                "line 3: tabs are not allowed",
            ),
            (
                "format: 1\nfmt: {indent: 2}\n",
                "line 2: only plain text values",
            ),
            (
                "format: 1\nfmt:\n  indent: \"2\"\n",
                "line 3: only plain text values",
            ),
            (
                "format: 1\nfmt:\n  indent: 2\n   width: 80\n",
                "line 4: inconsistent indentation",
            ),
            ("format: 1\nfmt\n", "line 2: expected `key: value`"),
            ("format: 1\n---\n", "line 2: document markers"),
        ] {
            let e = Config::parse(text).unwrap_err();
            assert!(e.starts_with(want), "{text:?}: {e}");
        }
    }

    #[test]
    fn the_old_toml_still_parses() {
        let c = Config::parse_toml("# top\n[fmt]\nindent = 2  # two\nwidth = 80\n").unwrap();
        assert_eq!(
            c.fmt,
            FmtConfig {
                indent: 2,
                width: 80
            }
        );
        assert!(Config::parse_toml("[fmt]\nindnt = 2\n").is_err());
    }

    #[test]
    fn the_closest_file_above_wins() {
        let base = std::env::temp_dir().join(format!("pelago-config-{}", std::process::id()));
        let inner = base.join("a").join("b");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(base.join("pelago.yaml"), "format: 1\nfmt:\n  indent: 3\n").unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 3);
        std::fs::write(
            base.join("a").join("pelago.yaml"),
            "format: 1\nfmt:\n  indent: 2\n",
        )
        .unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 2);
        // an old pelago.toml is read when no pelago.yaml shares its directory
        std::fs::remove_file(base.join("a").join("pelago.yaml")).unwrap();
        std::fs::write(base.join("a").join("pelago.toml"), "[fmt]\nindent = 5\n").unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 5);
        // pelago.yaml beats pelago.toml in the same directory
        std::fs::write(
            base.join("a").join("pelago.yaml"),
            "format: 1\nfmt:\n  indent: 6\n",
        )
        .unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 6);
        std::fs::write(
            base.join("a").join("pelago.yaml"),
            "format: 1\nfmt:\n  indent: 9\n",
        )
        .unwrap();
        assert!(Config::find(&inner.join("x.pel"))
            .unwrap_err()
            .contains("pelago.yaml"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
