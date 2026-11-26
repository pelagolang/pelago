//! `pelago.yaml`: the settings file for everything about the language that is not source code.
//! Today it has one section, `fmt`; others (the package, the build) will join it later.
//!
//! The file is the strict YAML subset of [`crate::yaml`], the same reader grammar as Islet's
//! files: block mappings, comments, plain or quoted strings, `true`, `false` and whole numbers.
//! The first key must be `format: 1`. Unknown sections and keys are errors, so a typo does not
//! silently do nothing. Errors name a line and column.
//!
//! A tool looks for the file from the directory of the file it works on, upwards ([`Config::find`]).

use crate::yaml::{self, Diag, PErr, Val};
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
    /// Reads the text of a `pelago.yaml`. An error reads `line:column: message`.
    pub fn parse(text: &str) -> Result<Config, String> {
        Config::read(text).map_err(|e| {
            let d = Diag::new("", text, e);
            format!("{}:{}: {}", d.line, d.col, d.msg)
        })
    }

    fn read(text: &str) -> Result<Config, PErr> {
        let table = yaml::parse(text)?;
        let mut config = Config::default();
        let Some(first) = table.entries.first() else {
            return Ok(config);
        };
        if first.key != "format" {
            return Err(PErr::new(first.key_at, "the first key must be `format: 1`"));
        }
        if first.node.v != Val::Int(1) {
            return Err(PErr::new(
                first.node.at,
                "unsupported `format` (this pelago reads `format: 1`)",
            ));
        }
        for e in &table.entries[1..] {
            match e.key.as_str() {
                "fmt" => {
                    let Val::Table(t) = &e.node.v else {
                        return Err(PErr::new(
                            e.node.at,
                            "`fmt` is a section; put its keys indented below it",
                        ));
                    };
                    for f in &t.entries {
                        let Val::Int(n) = f.node.v else {
                            return Err(PErr::new(
                                f.node.at,
                                format!("`{}` must be a number", f.key),
                            ));
                        };
                        let n = usize::try_from(n).unwrap_or(0);
                        match f.key.as_str() {
                            "indent" if (1..=8).contains(&n) => config.fmt.indent = n,
                            "indent" => {
                                return Err(PErr::new(f.node.at, "`indent` must be from 1 to 8"))
                            }
                            "width" if (20..=400).contains(&n) => config.fmt.width = n,
                            "width" => {
                                return Err(PErr::new(f.node.at, "`width` must be from 20 to 400"))
                            }
                            k => {
                                return Err(PErr::new(
                                    f.key_at,
                                    format!(
                                        "unknown key `{k}` in `fmt` (known: `indent`, `width`)"
                                    ),
                                ))
                            }
                        }
                    }
                }
                "format" => return Err(PErr::new(e.key_at, "`format` appears twice")),
                k => {
                    return Err(PErr::new(
                        e.key_at,
                        format!("unknown key `{k}` (known: `format`, `fmt`)"),
                    ))
                }
            }
        }
        Ok(config)
    }

    /// The `pelago.yaml` closest above `path` (a file or a directory), or the defaults when there
    /// is none. An error names the file that could not be read.
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
            let read =
                |f: &Path| std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()));
            if file.is_file() {
                return Config::parse(&read(&file)?)
                    .map_err(|e| format!("{}: {e}", file.display()));
            }
            dir = d.parent();
        }
        Ok(Config::default())
    }
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
    fn mistakes_are_errors_with_a_line_and_column() {
        for (text, want) in [
            ("format: 1\nfmt:\n  indnt: 2\n", "3:3: unknown key `indnt`"),
            ("format: 1\nformat: 1\n", "2:1: duplicate key `format`"),
            ("format: 1\nfromat: 1\n", "2:1: unknown key `fromat`"),
            (
                "fmt:\n  indent: 2\n",
                "1:1: the first key must be `format: 1`",
            ),
            ("format: 2\n", "1:9: unsupported `format`"),
            ("format: 1\n  indent: 2\n", "2:3: unexpected indentation"),
            (
                "format: 1\nfmt:\n  indent: 0\n",
                "3:11: `indent` must be from 1 to 8",
            ),
            (
                "format: 1\nfmt:\n  width: wide\n",
                "3:10: `width` must be a number",
            ),
            (
                "format: 1\nfmt:\n\tindent: 2\n",
                "3:1: tabs are not allowed",
            ),
            ("format: 1\nfmt: {indent: 2}\n", "2:6: flow style"),
            (
                "format: 1\nfmt:\n  indent: \"2\"\n",
                "3:11: `indent` must be a number",
            ),
            ("format: 1\nfmt: 3\n", "2:6: `fmt` is a section"),
            (
                "format: 1\nfmt:\n  width: yes\n",
                "3:10: `yes`: YAML tools disagree",
            ),
            (
                "format: 1\nfmt:\n  width: 1.5\n",
                "3:10: `1.5` would be read as a number",
            ),
            ("format: 1\nfmt\n", "2:1: expected `key: value`"),
            ("format: 1\n---\n", "2:1: document markers"),
        ] {
            let e = Config::parse(text).unwrap_err();
            assert!(e.starts_with(want), "{text:?}: {e}");
        }
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
