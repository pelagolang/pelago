# Commit messages and pull request titles

Every commit subject and every pull request title uses [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).
This holds in all four repositories: `pelago`, `tree-sitter-pelago`, `zed-pelago` and `vscode-pelago`.
Pull requests are squash-merged, so the PR title becomes the commit on `main`: get the title right and the history stays readable.

## Format

```
type(scope): summary

Body: what changed and why, wrapped at about 72 columns. Optional.

Footer: BREAKING CHANGE: ..., Refs: card A-11. Optional.
```

- `scope` is optional. In `pelago` use the stage or area: `lexer`, `parser`, `check`, `typecheck`, `lower`, `ir`, `opt`, `native`, `lsp`, `fmt`, `editors`, `bench`, `docs`.
- `summary` is imperative, starts with a lowercase letter, has no trailing period, and keeps the whole first line under 72 characters.
- Write `!` after the type or scope (`feat(parser)!: ...`) or add a `BREAKING CHANGE:` footer when existing `.pel` programs or the command line stop working.

## Types

| type | use it for |
|---|---|
| `feat` | a new language feature, command or capability |
| `fix` | something was wrong, now it is right (the branch type may be `bug` or `fix`; the commit type is `fix`) |
| `perf` | faster code, a faster compiler, or smaller output |
| `refactor` | no change in behaviour |
| `docs` | docs, plans, comments only |
| `test` | tests only |
| `chore` | tooling and housekeeping that fits nowhere else |
| `ci` | GitHub workflows and other CI files |
| `build` | `Cargo.toml`, build scripts, packaging |
| `revert` | undoing an earlier commit (git's own `Revert "..."` subject is accepted too) |

## Examples

```
perf(native): divide by constants with a multiply
fix(lower): give each shadowing let its own slot
feat(parser): add step to for ranges
docs: add a pull request template and scenarios
ci: lint pull request titles
feat(parser)!: reject the old `class` keyword
```

## How it is checked

- `.github/workflows/conventional-commits.yml` lints the PR title and the subject of every non-merge commit in the PR.
- Rename a PR to fix its title; the check runs again on its own. Reword a commit with `git commit --amend` (your own branch only) or fix the PR title and squash-merge.
- Branch names are separate: `<author>/<type>/<short-name>`, for example `claude/perf/const-division`.
