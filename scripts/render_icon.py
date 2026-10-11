#!/usr/bin/env python3
"""Render assets/lumen.svg to assets/lumen.png (256x256) and assets/lumen.ico.

Both are committed, so this only runs again when the SVG changes.
Needs librsvg through PyGObject (on Ubuntu: gir1.2-rsvg-2.0) and Pillow.
The .ico holds the sizes Windows picks from for the .exe in File Explorer.
"""
import pathlib

import gi
from PIL import Image

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402

assets = pathlib.Path(__file__).resolve().parent.parent / "assets"

# The SVG is drawn at 256x256, so its own size is the PNG size.
pixbuf = Rsvg.Handle.new_from_file(str(assets / "lumen.svg")).get_pixbuf()
pixbuf.savev(str(assets / "lumen.png"), "png", [], [])

ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
Image.open(assets / "lumen.png").save(assets / "lumen.ico", sizes=[(n, n) for n in ICO_SIZES])
