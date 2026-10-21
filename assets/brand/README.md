# Pelago brand assets

The logo, icons, file icons and social images for Pelago. Design record and rules: [`docs/design/brand.md`](../../docs/design/brand.md).

The SVG files are the source. The PNG, ICO and ICNS files are exported from them (headless Chromium for the PNGs, ImageMagick for the ICO, a small packer for the ICNS). The wordmark in every logo SVG is outlined to paths, so no font is needed to display them.

## Layout

| folder | contents |
|---|---|
| `logo/` | `pelago-mark`, `pelago-horizontal`, `pelago-stacked`, each as colour (`.svg`, for light backgrounds), `-dark` (lagoon, for dark backgrounds), `-black` and `-white` (one colour). PNGs on a transparent background: mark 1024, horizontal 2000 wide, stacked 1200 wide. |
| `icon/` | App icon `pelago-icon.svg` (teal badge) and `pelago-icon-dark.svg`; PNG ladder 16 to 1024; `favicon.svg`, `favicon.ico` (16, 32, 48), `pelago.ico` (16, 32, 48, 256), `pelago.icns`; `apple-touch-icon-180.png`, `android-chrome-192.png`, `android-chrome-512.png`, `maskable-512.png`, `mstile-150.png`; round avatars `avatar-512.png` and `avatar-dark-512.png`. |
| `editor/` | `vscode-icon-256.png` and `file-icons/`: the `.pel` source file icon in four styles (`outline`, `glyph`, `solid`, `band`) and the `pelago.toml` manifest icon in three (`chart`, `solid`, `anchor`), each with a `-dark` variant for dark themes, as SVG and 128px PNG. |
| `social/` | `github-social-1280x640.png`, `og-1200x630.png`, `x-header-1500x500.png`, `linkedin-1584x396.png`, `readme-banner-1280x320.png`. |

## Quick rules

- Name: lowercase `pelago` in the logo, `Pelago` in prose.
- Colours: Pelago Teal `#0E7C86`, Lagoon `#5FD0C8`, Deep Sea `#0B2239`, Foam `#F4F6F8`. Use Deep Sea, not teal, for small text on white.
- Clear space: one dot diameter on every side. Minimum size: mark 24px (use the badge icon below that), horizontal lockup 96px wide.
- Do not stretch, rotate, recolour, outline, add effects to, or put the logo on a low-contrast background.
- Fonts: wordmark in Comfortaa Bold, text in Outfit (both SIL Open Font License). The fonts are not in this repository.
- Editor extensions use the icons only. They never change the user's theme or workbench colours.

The `.ico` and `.icns` files bundle several sizes. The `.icns` holds 128, 256, 512 and 1024 PNGs, which covers the current macOS sizes but not the small legacy ones.
