# Orca Bridge identity

An original connection mark for Orca Bridge: two interwoven bands bridge the gap
between local tools and ongoing work. The continuous teal span crosses an indigo
return span, with transparent cuts that preserve the weave in a single color.
The concept was explored with built-in image generation and rebuilt as four
editable Bézier paths. The shipped artwork contains no raster image or effects.

## Masters and exports

- `mark.svg`: editable 256 × 256 vector master, on a transparent canvas.
- `wordmark.svg`: editable outlined lettering master, without font dependencies.
- `logo-light.svg`: generated horizontal lockup with dark lettering.
- `logo-dark.svg`: generated horizontal lockup with inverse lettering.
- [Raycast icon](../../integrations/raycast/assets/icon.png): 256 px PNG rendered
  from the mark; [render command](../../integrations/raycast/assets/README.md).

Run `make artwork` to compose both 680 × 208 lockups from the two masters. The
README selects the appearance with its `picture` element and displays it at
600 px wide. Edit the masters rather than the generated lockups.

## Construction and use

The lockup places the mark at 75% scale and the lettering at its native size.
Keep the spacing and aspect ratio intact. Give the standalone mark at least
24 master units of clearspace when placing it beside other artwork.

The mark has been rendered and inspected at 16, 32, 64, 128, 256, 512 and 1024 px
on light and dark surfaces. Use 32 px or larger for the standalone mark; 16 px
retains the silhouette as a small indicator. The horizontal lockups have been
checked at 300 and 600 px wide. Single-color and checkerboard proofs were also
inspected. All exterior and interior negative spaces remain transparent.

Use `logo-light.svg` on light surfaces and `logo-dark.svg` on dark surfaces. For
one-color use, replace both mark group fills with the same contrasting ink.
Keep the transparent cuts; do not add a tile, gradients, glow or shadows.

## Palette

| Role | Color | HEX |
| --- | --- | --- |
| Forward span | Deep teal | `#118b98` |
| Return span | Indigo | `#596de0` |
| Light-page lettering | Ink | `#202533` |
| Dark-page lettering | Frost | `#eef2ff` |

The span colors are artwork colors, not a prescription for small UI text.

## Typography and provenance

The exact name is **Orca Bridge**. The outlined wordmark uses Space Grotesk Bold
for “Orca” and Medium for “Bridge”, at 78 units with −1.5 units of tracking.
Additional pair adjustments are −1 unit for `Or` and `Br`, and −0.5 units for
`rc`, `ca`, `ri`, `id`, `dg` and `ge`. The baseline is at 82 units in the lettering
master. The lockup translates that master by 228 units horizontally and 46
vertically. Reuse the outlined master rather than substituting live text.

Typeface source: [Space Grotesk, revision 03507d0](https://github.com/floriankarsten/space-grotesk/tree/03507d024a01282884232081fc6011c09ff4e849),
by the Space Grotesk Project Authors, under the SIL Open Font License 1.1.
The [font license](wordmark-license.txt) is included; no font binaries are shipped.
The original mark was authored for this repository on 2026-10-22 and is covered
by the repository's [MIT license](../../LICENSE).

Each SVG includes accessible names and descriptions for its visual layers.
Supply context-appropriate `alt` text when embedding the artwork as an image.
