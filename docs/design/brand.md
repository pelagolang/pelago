# Brand

Status: decided with the owner, 2026-10-21. Assets live in `assets/brand/` (see its `README.md` for the file list).

## Decisions

- **Mark:** a monoline capital P in a round-capped stroke, a dot to its right on the baseline (the island, and the full stop of a statement), and a single wave under the P (the sea).
  Candidates considered: three islands on a waterline, a P with waves cut through the stem, a bar-and-dome P, several font-based P variants (all rejected: sans and serif letterforms were too generic or too dev-flavoured).
- **Name:** lowercase `pelago`, set in Comfortaa Bold, outlined to paths. Capital `Pelago` is used in running text.
- **Text and UI font:** Outfit. Code stays in whatever monospace the reader uses.
- **Palette:** Pelago Teal `#0E7C86`, Lagoon `#5FD0C8` (on dark), Deep Sea `#0B2239`, Foam `#F4F6F8`. Teal on white is 4.9:1: fine for the logo and large text, not for small body text.
- **Icons:** a rounded badge (teal with a white mark, or Deep Sea with a Lagoon mark). Below 24px the badge replaces the loose mark.
- **File icons:** `.pel` has four styles (outline, glyph, solid, band) and `pelago.toml` three (chart, solid, anchor), shipped side by side so editor and OS integrations can pick. The family pairing we recommend is `pel-solid` with `pelago-toml-solid`.

## Rules

- Editors keep their own theme: extensions ship icons, never a theme or workbench colours.
- The wordmark in logo SVGs is outlined; do not retype it in a font.
- Teal `#0E7C86` on `#0B2239` is 3.3:1 (graphics only); use Lagoon on dark backgrounds.

## Open items

- A domain, tagline and slide copy are placeholders (the social images say "A self-hosting systems language.").
- The macOS `.icns` holds only the 128 to 1024 sizes.
- `vscode-pelago`, `zed-pelago` and `tree-sitter-pelago` do not use these files yet; wiring the icons in is separate work in those repositories.
