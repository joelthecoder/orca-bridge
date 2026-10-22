# Launcher artwork

`icon.png` is a transparent 256 × 256 PNG rendered from Orca Bridge's original
[SVG master](../../../Assets/README/mark.svg). Teal and indigo bands form an
interwoven connection, with transparent cuts that also work in one color.

The mark was authored for this repository on 2026-10-22. The concept was explored
with built-in image generation and rebuilt as editable vector paths. The artwork
is covered by the [MIT license](../../../LICENSE); its
[identity guide](../../../Assets/README/README.md) documents construction and use.

Render from the repository root with librsvg:

```sh
rsvg-convert --width 256 --height 256 Assets/README/mark.svg \
  --output integrations/raycast/assets/icon.png
```
