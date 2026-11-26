//! The toolchain version and its channel.
//!
//! One version covers the compiler, interpreter, `std` and the CLI. The channel is read from the
//! version text: `-nightly.`, `-beta.` and `-rc.` mark the other channels, a plain `X.Y.Z` is stable.

/// The version this binary was built as (`PELAGO_BUILD_VERSION` or the crate version).
pub const VERSION: &str = env!("PELAGO_VERSION");
/// Short commit hash, or `unknown` outside a git checkout.
pub const COMMIT: &str = env!("PELAGO_COMMIT");
/// Commit date (`YYYY-MM-DD`), or `unknown`.
pub const COMMIT_DATE: &str = env!("PELAGO_COMMIT_DATE");

/// A release channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
    Rc,
    Nightly,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
            Channel::Rc => "rc",
            Channel::Nightly => "nightly",
        }
    }

    pub fn parse(s: &str) -> Option<Channel> {
        match s {
            "stable" => Some(Channel::Stable),
            "beta" => Some(Channel::Beta),
            "rc" => Some(Channel::Rc),
            "nightly" => Some(Channel::Nightly),
            _ => None,
        }
    }
}

/// The channel a version string belongs to.
pub fn channel_of(version: &str) -> Channel {
    if version.contains("-nightly.") {
        Channel::Nightly
    } else if version.contains("-beta.") {
        Channel::Beta
    } else if version.contains("-rc.") {
        Channel::Rc
    } else {
        Channel::Stable
    }
}

/// The text `pelago --version` prints: the plain version on stable, with the commit and date on
/// every other build.
pub fn version_line_for(version: &str, commit: &str, date: &str) -> String {
    if channel_of(version) == Channel::Stable {
        format!("pelago {version}")
    } else {
        format!("pelago {version} ({commit} {date})")
    }
}

pub fn version_line() -> String {
    version_line_for(VERSION, COMMIT, COMMIT_DATE)
}

/// The longer form for `pelago --version --verbose`.
pub fn version_verbose() -> String {
    format!(
        "{}\nchannel: {}\ncommit: {}\ndate: {}\ntarget: {}-{}\nprofile: {}",
        version_line(),
        channel_of(VERSION).name(),
        COMMIT,
        COMMIT_DATE,
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_come_from_the_version_text() {
        assert_eq!(channel_of("0.1.0"), Channel::Stable);
        assert_eq!(channel_of("0.2.0-beta.3"), Channel::Beta);
        assert_eq!(channel_of("0.2.0-rc.1"), Channel::Rc);
        assert_eq!(
            channel_of("0.2.0-nightly.20261126+abc1234"),
            Channel::Nightly
        );
    }

    #[test]
    fn only_non_stable_builds_show_commit_and_date() {
        assert_eq!(
            version_line_for("0.1.0", "abc1234", "2026-11-26"),
            "pelago 0.1.0"
        );
        assert_eq!(
            version_line_for("0.2.0-rc.1", "abc1234", "2026-11-26"),
            "pelago 0.2.0-rc.1 (abc1234 2026-11-26)"
        );
    }

    #[test]
    fn channel_names_round_trip() {
        for c in [
            Channel::Stable,
            Channel::Beta,
            Channel::Rc,
            Channel::Nightly,
        ] {
            assert_eq!(Channel::parse(c.name()), Some(c));
        }
        assert_eq!(Channel::parse("dev"), None);
    }
}
