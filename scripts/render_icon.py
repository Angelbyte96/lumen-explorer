#!/usr/bin/env python3
"""Render assets/lumen.svg to assets/lumen.png (256x256).

The PNG is committed, so this only runs again when the SVG changes.
Needs librsvg through PyGObject (on Ubuntu: gir1.2-rsvg-2.0).
"""
import pathlib

import gi

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402

assets = pathlib.Path(__file__).resolve().parent.parent / "assets"

# The SVG is drawn at 256x256, so its own size is the PNG size.
pixbuf = Rsvg.Handle.new_from_file(str(assets / "lumen.svg")).get_pixbuf()
pixbuf.savev(str(assets / "lumen.png"), "png", [], [])
