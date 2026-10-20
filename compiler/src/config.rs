//! `pelago.toml`: the settings file for everything about the language that is not source code.
//! Today it has one section, `[fmt]`; others (the package, the build) will join it later.
//!
//! The file is a small subset of TOML, read without a library: `# comments`, `[section]` lines
//! and `key = value` lines where a value is an integer, a quoted string or `true` / `false`.
//! Unknown sections and keys are errors, so a typo does not silently do nothing.
//!
//! A tool looks for the file from the directory of the file it works on, upwards ([`Config::find`]).

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
    /// Reads the text of a `pelago.toml`.
    pub fn parse(text: &str) -> Result<Config, String> {
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

    /// The `pelago.toml` closest above `path` (a file or a directory), or the defaults when there
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
            let file = d.join("pelago.toml");
            if file.is_file() {
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                return Config::parse(&text).map_err(|e| format!("{}: {e}", file.display()));
            }
            dir = d.parent();
        }
        Ok(Config::default())
    }
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
        let c = Config::parse("# top\n[fmt]\nindent = 2  # two\nwidth = 80\n").unwrap();
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
            ("[fmt]\nindnt = 2\n", "line 2: unknown key `indnt`"),
            ("[format]\n", "line 1: unknown section `[format]`"),
            ("indent = 2\n", "line 1: `indent` is outside any section"),
            (
                "[fmt]\nindent = 0\n",
                "line 2: `indent` must be from 1 to 8",
            ),
            ("[fmt]\nwidth = wide\n", "line 2: `width` must be a number"),
            ("[fmt\n", "line 1: a section header must end with `]`"),
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
        std::fs::write(base.join("pelago.toml"), "[fmt]\nindent = 3\n").unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 3);
        std::fs::write(base.join("a").join("pelago.toml"), "[fmt]\nindent = 2\n").unwrap();
        assert_eq!(Config::find(&inner.join("x.pel")).unwrap().fmt.indent, 2);
        std::fs::write(base.join("a").join("pelago.toml"), "[fmt]\nindent = 9\n").unwrap();
        assert!(Config::find(&inner.join("x.pel"))
            .unwrap_err()
            .contains("pelago.toml"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
