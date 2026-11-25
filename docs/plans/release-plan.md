# Release plan: versions, channels, pipeline, `pelago self update`

Decided by Dennis on 2026-11-25. Card: [B-06](cards/B-06-releases.md). This file is the plan; the pipeline, the updater and the website pages are built in the PRs listed at the end.

## 1. Versioning policy

- **Strict semver** (`MAJOR.MINOR.PATCH`). First version: **0.1.0**.
- **0.x** means breaking changes are allowed in any minor release. **1.0.0 is cut when Freeze opens** (see `docs/process/workflow.md`); from then on breaking changes need a major.
- **One version for the whole toolchain**: compiler, interpreter, `std`, `core` and the `pelago` CLI. It is the `version` in `compiler/Cargo.toml` and the git tag `vX.Y.Z` in `pelagolang/pelago`.
- **Islet has its own version** (its own repo and tags).
- The editor extensions, `tree-sitter-pelago` and the website **follow the language version they support**: each states "supports Pelago 0.x" and is released when the grammar or the CLI contract changes, not on every compiler release.
- Changelog: generated from Conventional Commits (`feat` and `fix` listed, `!` and `BREAKING CHANGE:` first).

## 2. Targets

| OS | Architectures | Archive |
|---|---|---|
| Linux | x86-64, AArch64 | `.tar.gz` |
| macOS | Intel (x86-64), Apple silicon (AArch64) | `.tar.gz` |
| Windows | x86-64, ARM64 | `.zip` |

Names: `pelago-<version>-<arch>-<os>.<ext>` (for example `pelago-0.1.0-x86_64-linux.tar.gz`). Each archive holds one single static binary `pelago` (`pelago.exe`), the licences and a README. Every release also carries `SHA256SUMS`, and `SHA256SUMS.sig` (section 5).

Linux binaries are built on the same job that runs CI for that architecture (`ubuntu-latest`, `ubuntu-24.04-arm`); Windows on `windows-latest` and `windows-11-arm`; macOS on `macos-13` (Intel) and `macos-latest` (Apple silicon). No external dependencies, so static linking only needs the Rust musl target on Linux.

## 3. Channels

| Channel | Tag / version | Cut from | Rules |
|---|---|---|---|
| **stable** | `vX.Y.Z` | the rc commit, or `release/X.Y` for patches | Only one release is marked **Latest** on GitHub. |
| **beta** | `vX.Y.0-beta.N` | branch `release/X.Y` | Fixes only after the branch is cut. Marked pre-release. |
| **rc** | `vX.Y.0-rc.N` | `release/X.Y` | Soaks **at least one month**, then the *same commit* is promoted to stable. Any blocker resets the clock with a new `rc.N`. Marked pre-release. |
| **nightly** | `X.Y+1.0-nightly.YYYYMMDD+<sha7>` (for example `0.2.0-nightly.20261126+abc1234`) | `main` | Built daily, only if `main` changed since the last nightly **and** CI is green on that commit. One rolling pre-release (tag `nightly`) holding the last 14 builds. Turned on right away, before 0.1.0. |

Patch releases (`vX.Y.Z`, Z > 0) go straight to stable from `release/X.Y`, with no beta or rc.

Flow for a minor: `main` -> cut `release/0.2` -> `v0.2.0-beta.1` ... -> `v0.2.0-rc.1` -> one month with no blocker -> `v0.2.0` (same commit as the last rc; no rebuild of the source, the stable build is a fresh build of the identical commit and its checksum is expected to match when builds are reproducible).

## 4. Pipeline (GitHub Actions)

- `release.yml`: triggered by a pushed tag `v*`. A build matrix produces the six archives, a job gathers them, writes `SHA256SUMS`, signs it, generates release notes from Conventional Commits since the previous tag of the same channel, and creates the GitHub release. A tag with `-beta.`, `-rc.` or `-nightly.` is marked pre-release; only a plain `vX.Y.Z` can be Latest.
- `nightly.yml`: scheduled daily. It compares `main`'s head with the last nightly's commit, checks that CI is green on that head, then builds the same matrix, replaces the rolling `nightly` release assets and prunes to the newest 14.
- `release-check`: a CI job that verifies the tag matches `compiler/Cargo.toml` and that the changelog generator runs.
- A helper script (`tools/release/`, in Python, no new Rust dependency) is the single place that knows the version rules, so the workflows, `self update` tests and docs agree.

## 5. Signing and verification

- Release binaries are signed with **Ed25519**, matching the Islet signing design. The checksum file `SHA256SUMS` is signed (`SHA256SUMS.sig`); the public key is committed in the repo (`docs/release/pelago-release.pub`) and printed on the website, and is baked into the `pelago` binary so `self update` verifies without a network trip to fetch the key.
- The private key lives only in a GitHub Actions secret (`RELEASE_SIGNING_KEY`). **Dennis must create it**; until it exists the workflow builds and publishes checksums but fails closed on signing for stable tags. Key rotation: the new public key is added to the binary list in a release signed by the old key.
- **Windows binaries are not Authenticode-signed at first.** SmartScreen will show an "unknown publisher" warning on first run. This is documented on the install and verification pages, together with how to check the checksum and the signature (`Get-FileHash -Algorithm SHA256`, then `pelago self verify` or the standalone check). Authenticode is revisited when there is a certificate budget.
- macOS binaries are not notarised at first either (Gatekeeper: `xattr -d com.apple.quarantine`); documented the same way.

## 6. `pelago --version`

- Stable: `pelago 0.1.0`.
- Any other build: `pelago 0.2.0-nightly.20261126+abc1234 (abc1234 2026-11-26)`, that is version, short commit hash, commit date. The hash and date are embedded at build time by a `build.rs` that reads `git`; outside a git checkout it prints `unknown`. `pelago --version --verbose` also prints target, channel and build profile.

## 7. `pelago self update`

- `pelago self update [--channel stable|beta|rc|nightly] [--check] [--version X.Y.Z]`.
- The channel is remembered in the install directory (`~/.pelago/channel`, `%LOCALAPPDATA%\pelago\channel`) so a plain `self update` stays on it; the default is `stable`.
- It fetches the release list from the GitHub API, picks the newest release for the channel (stable: Latest; beta, rc: highest matching tag; nightly: the rolling release), downloads the archive for the running target and `SHA256SUMS` plus `.sig`, verifies the signature with the baked-in key and the hash, then replaces the running binary (on Windows: rename the running exe aside, move the new one in, delete the old one at next start).
- HTTP without a Rust dependency: the CLI shells out to the system's `curl` (Linux, macOS, Windows 10+ all ship it) for the first version; a native client over the std networking stack is a later improvement. Anything that fails leaves the old binary in place.
- No downgrade without `--version`. `--check` only prints what would be installed.

## 8. Install scripts

- `install.sh` (Linux, macOS) and `install.ps1` (Windows PowerShell), served from the website and kept in `pelagolang/website` and `pelagolang/pelago` `tools/install/`. Both take the same channel option: `sh install.sh --channel nightly`, and `.\install.ps1 -Channel nightly`.
- They detect OS and architecture, download the archive and checksum for the channel, verify the hash (and the signature when `pelago` can verify it), install to `~/.pelago/bin` or `%LOCALAPPDATA%\pelago\bin`, and print the PATH line. PowerShell one-liner: `irm https://pelago.dev/install.ps1 | iex` (the real URL is set once the website serves it).

## 9. Website pages (one per feature)

Channels, versioning policy, installing, `self update`, verifying signatures (and the Windows signing note). Each linked from the docs sidebar under a "Releases" group; verified on the live site after deploy.

## 10. Order of work

1. **This PR**: plan, card, board, ROADMAP row.
2. Pipeline: `release.yml`, `nightly.yml`, release helper script, `--version` with hash and date, version bump to 0.1.0 preparation (the 0.1.0 tag itself waits for Dennis).
3. `pelago self update` and the install scripts.
4. Website pages, verified live.

## 11. Open items for Dennis

1. Create the Ed25519 key pair and add the `RELEASE_SIGNING_KEY` secret (Claude will provide a PowerShell-friendly command and commit the public key).
2. Which domain serves the install scripts (placeholder until the website decides).
3. When to tag 0.1.0 (nightly runs from the moment the workflow merges).
