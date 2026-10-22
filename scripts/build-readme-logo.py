#!/usr/bin/env python3
"""Compose transparent README wordmarks from the repository's SVG master."""

from pathlib import Path
import xml.etree.ElementTree as ET


root = Path(__file__).resolve().parent.parent
assets = root / "Assets" / "README"
ET.register_namespace("", "http://www.w3.org/2000/svg")
master = ET.parse(assets / "mark.svg").getroot()
wordmark = ET.parse(assets / "wordmark.svg").getroot().find("{http://www.w3.org/2000/svg}g")
if wordmark is None:
    raise ValueError("wordmark.svg must contain a lettering group")
emblem = "\n".join(
    ET.tostring(child, encoding="unicode").strip()
    for child in master
    if child.tag.endswith("}g")
)

for appearance, ink in [("light", "#202533"), ("dark", "#eef2ff")]:
    wordmark.set("fill", ink)
    lettering = ET.tostring(wordmark, encoding="unicode").strip()
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" id="orca-bridge-logo-{appearance}" width="680" height="208" viewBox="0 0 680 208" role="graphics-document document" aria-labelledby="logo-title" aria-describedby="logo-description">
  <title id="logo-title">Orca Bridge</title>
  <desc id="logo-description">An interwoven connection mark beside the Orca Bridge wordmark.</desc>
  <g id="mark-placement" role="none" transform="translate(8 8) scale(0.75)">
{emblem}
  </g>
  <g id="wordmark-placement" role="none" transform="translate(228 46)">
{lettering}
  </g>
</svg>
'''
    (assets / f"logo-{appearance}.svg").write_text(svg)
