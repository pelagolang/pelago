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

- **Tagline:** "A self-hosting systems language." No domain appears in any asset.
- **Wordmark kerning:** the outlines carry the font's one kerning pair that matters here (`l` then `a`, 30/1000 em tighter). We checked the outlined lockup against the live font at 2000px and they overlap exactly.

## Build

`assets/brand/build/build.py` regenerates every SVG, the sprite, the PNGs, the ICO and ICNS files and the social images (see `assets/brand/README.md`). The SVGs are the source of truth.

## Open items

- A shorter, capability-showing tagline is an open idea (for example "Native code. No GC. No runtime."); the current one stays until the owner picks.
- `vscode-pelago`, `zed-pelago` and `tree-sitter-pelago` take the icons from this directory (copied in, with the source named in each repo).
