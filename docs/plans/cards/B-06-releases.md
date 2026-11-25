Id: B-06
Title: Releases, channels and `pelago self update`
Column: Doing
Order: -
Depends on: -
Size: L
Touches: workflows, tools/release, compiler main.rs (version, self), install scripts, website

# Why

Dennis (2026-11-25): strict semver from 0.1.0, one version for the toolchain, stable/beta/rc/nightly channels, signed binaries for six targets, and an updater. Nightly first.

# Plan

[release-plan.md](../release-plan.md) has the policy, targets, channels, signing, `--version`, `self update`, install scripts and the order of PRs.

# Done means

- [ ] Plan merged (this PR)
- [ ] `release.yml` and `nightly.yml` run; nightly has produced a build
- [ ] `pelago --version` prints hash and date on non-stable builds
- [ ] `pelago self update --channel ...` and both install scripts work on all targets
- [ ] Website pages live and checked on the live site
