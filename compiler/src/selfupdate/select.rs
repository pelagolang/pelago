//! Choosing a release: versions, their order, and the newest build on a channel.

use super::json::Json;
use crate::version::Channel;
use std::cmp::Ordering;

/// A parsed version: `X.Y.Z`, `X.Y.Z-beta.N`, `X.Y.Z-rc.N` or `X.Y.Z-nightly.YYYYMMDD+sha`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub base: (u64, u64, u64),
    /// 0 nightly, 1 beta, 2 rc, 3 stable: the order of the channels for one base version.
    pub kind: u8,
    pub num: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next()?;
        let (base, pre) = match text.split_once('-') {
            Some((b, p)) => (b, Some(p)),
            None => (text, None),
        };
        let mut it = base.split('.').map(|n| n.parse::<u64>().ok());
        let base = (it.next()??, it.next()??, it.next()??);
        if it.next().is_some() {
            return None;
        }
        let (kind, num) = match pre {
            None => (3, 0),
            Some(p) => {
                let (name, n) = p.split_once('.')?;
                let n = n.parse().ok()?;
                match name {
                    "nightly" => (0, n),
                    "beta" => (1, n),
                    "rc" => (2, n),
                    _ => return None,
                }
            }
        };
        Some(Version { base, kind, num })
    }

    pub fn channel(&self) -> Channel {
        match self.kind {
            0 => Channel::Nightly,
            1 => Channel::Beta,
            2 => Channel::Rc,
            _ => Channel::Stable,
        }
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.base, self.kind, self.num).cmp(&(o.base, o.kind, o.num))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
    /// Where the full asset list lives (the releases list may cut a long one short).
    pub assets_url: Option<String>,
}

/// The non-draft releases in the JSON the GitHub releases API returns.
pub fn parse_releases(json: &Json) -> Vec<Release> {
    let mut out = Vec::new();
    for r in json.arr() {
        if r.get("draft").and_then(Json::bool) == Some(true) {
            continue;
        }
        let Some(tag) = r.get("tag_name").and_then(Json::str) else {
            continue;
        };
        let assets = r
            .get("assets")
            .map(|a| a.arr())
            .unwrap_or(&[])
            .iter()
            .filter_map(|a| {
                Some(Asset {
                    name: a.get("name")?.str()?.to_string(),
                    url: a.get("browser_download_url")?.str()?.to_string(),
                })
            })
            .collect();
        out.push(Release {
            tag: tag.to_string(),
            prerelease: r.get("prerelease").and_then(Json::bool) == Some(true),
            assets,
            assets_url: r.get("assets_url").and_then(Json::str).map(str::to_string),
        });
    }
    out
}

/// What to download for one build.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub version: String,
    pub archive: Asset,
    pub sums: Asset,
    pub sig: Option<Asset>,
}

fn archive_ext(target: &str) -> &'static str {
    if target.ends_with("windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

fn candidate_in(
    release: &Release,
    version: &str,
    target: &str,
    shared_sums: bool,
) -> Option<Candidate> {
    let archive_name = format!("pelago-{version}-{target}.{}", archive_ext(target));
    let sums_name = if shared_sums {
        "SHA256SUMS".to_string()
    } else {
        format!("SHA256SUMS-{version}")
    };
    let find = |n: &str| release.assets.iter().find(|a| a.name == n).cloned();
    Some(Candidate {
        version: version.to_string(),
        archive: find(&archive_name)?,
        sums: find(&sums_name)?,
        sig: find(&format!("{sums_name}.sig")),
    })
}

/// Every build of `target` in the releases, with its parsed version.
fn all_candidates(releases: &[Release], target: &str) -> Vec<(Version, Candidate)> {
    let mut pool = Vec::new();
    let suffix = format!("-{target}.{}", archive_ext(target));
    for r in releases {
        if r.tag == "nightly" {
            // The rolling release holds many builds; each asset name carries its version.
            for a in &r.assets {
                let Some(version) = a
                    .name
                    .strip_prefix("pelago-")
                    .and_then(|n| n.strip_suffix(&suffix))
                else {
                    continue;
                };
                if let (Some(v), Some(c)) = (
                    Version::parse(version),
                    candidate_in(r, version, target, false),
                ) {
                    pool.push((v, c));
                }
            }
        } else if let (Some(v), Some(c)) = (
            Version::parse(&r.tag),
            candidate_in(r, &r.tag[1..], target, true),
        ) {
            pool.push((v, c));
        }
    }
    pool
}

/// The newest build on `channel` for `target` (for example `x86_64-linux`), or the exact `want` version.
pub fn pick(
    releases: &[Release],
    channel: Channel,
    target: &str,
    want: Option<&str>,
) -> Option<Candidate> {
    let pool = all_candidates(releases, target);
    match want {
        Some(w) => {
            let w = w.trim_start_matches('v');
            pool.into_iter()
                .find(|(_, c)| c.version == w)
                .map(|(_, c)| c)
        }
        None => pool
            .into_iter()
            .filter(|(v, _)| v.channel() == channel)
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, c)| c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, pre: bool, assets: &[impl AsRef<str>]) -> Release {
        Release {
            tag: tag.into(),
            prerelease: pre,
            assets_url: None,
            assets: assets
                .iter()
                .map(|n| Asset {
                    name: n.as_ref().to_string(),
                    url: format!("https://x/{tag}/{}", n.as_ref()),
                })
                .collect(),
        }
    }

    fn stable_assets(v: &str) -> Vec<String> {
        vec![
            format!("pelago-{v}-x86_64-linux.tar.gz"),
            "SHA256SUMS".into(),
            "SHA256SUMS.sig".into(),
        ]
    }

    fn releases() -> Vec<Release> {
        let a = stable_assets("0.1.0");
        let b = stable_assets("0.1.1");
        let c = stable_assets("0.2.0-rc.1");
        let d = stable_assets("0.2.0-rc.2");
        vec![
            rel("v0.1.1", false, &b),
            rel("v0.1.0", false, &a),
            rel("v0.2.0-rc.1", true, &c),
            rel("v0.2.0-rc.2", true, &d),
            rel(
                "nightly",
                true,
                &[
                    "pelago-0.2.0-nightly.20261125+aaaaaaa-x86_64-linux.tar.gz",
                    "SHA256SUMS-0.2.0-nightly.20261125+aaaaaaa",
                    "pelago-0.2.0-nightly.20261126+bbbbbbb-x86_64-linux.tar.gz",
                    "SHA256SUMS-0.2.0-nightly.20261126+bbbbbbb",
                    "SHA256SUMS-0.2.0-nightly.20261126+bbbbbbb.sig",
                ],
            ),
        ]
    }

    #[test]
    fn versions_order_by_base_then_channel_then_number() {
        let p = |s| Version::parse(s).unwrap();
        assert!(p("0.2.0-nightly.20261201+abc1234") < p("0.2.0-beta.1"));
        assert!(p("0.2.0-beta.2") < p("0.2.0-rc.1"));
        assert!(p("0.2.0-rc.9") < p("0.2.0"));
        assert!(p("0.2.0") < p("0.2.1-nightly.20261201+abc1234"));
        assert!(p("0.1.1") > p("0.1.0"));
        assert!(p("0.10.0") > p("0.9.0"));
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3-weird.1"), None);
    }

    #[test]
    fn each_channel_picks_its_newest() {
        let r = releases();
        let t = "x86_64-linux";
        assert_eq!(pick(&r, Channel::Stable, t, None).unwrap().version, "0.1.1");
        assert_eq!(
            pick(&r, Channel::Rc, t, None).unwrap().version,
            "0.2.0-rc.2"
        );
        assert_eq!(
            pick(&r, Channel::Nightly, t, None).unwrap().version,
            "0.2.0-nightly.20261126+bbbbbbb"
        );
        assert!(pick(&r, Channel::Beta, t, None).is_none());
        assert!(pick(&r, Channel::Stable, "aarch64-linux", None).is_none());
    }

    #[test]
    fn the_checksum_files_follow_the_channel() {
        let r = releases();
        let n = pick(&r, Channel::Nightly, "x86_64-linux", None).unwrap();
        assert_eq!(n.sums.name, "SHA256SUMS-0.2.0-nightly.20261126+bbbbbbb");
        assert!(n.sig.is_some());
        let s = pick(&r, Channel::Stable, "x86_64-linux", None).unwrap();
        assert_eq!(s.sums.name, "SHA256SUMS");
        assert_eq!(s.sig.unwrap().name, "SHA256SUMS.sig");
    }

    #[test]
    fn an_exact_version_can_be_asked_for() {
        let r = releases();
        let c = pick(&r, Channel::Stable, "x86_64-linux", Some("0.1.0")).unwrap();
        assert_eq!(c.version, "0.1.0");
        let c = pick(&r, Channel::Stable, "x86_64-linux", Some("v0.2.0-rc.1")).unwrap();
        assert_eq!(c.version, "0.2.0-rc.1");
        assert!(pick(&r, Channel::Stable, "x86_64-linux", Some("9.9.9")).is_none());
    }
}
