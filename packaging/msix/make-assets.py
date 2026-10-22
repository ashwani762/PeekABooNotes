# Copyright (C) 2026 Ashwani Kumar Bachchan
# SPDX-License-Identifier: GPL-3.0-or-later

"""Render the MSIX logos (packaging/msix/Assets) from the same geometry as the in-code
app icon (`gfx::app_icon` in src/render/gfx.rs): a gold rounded square with three lines,
on a 32-unit grid. Needs Pillow only. Run: python packaging/msix/make-assets.py
"""
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent / "Assets"
SS = 8  # supersampling, for anti-aliased edges


def rgb(c, a=1.0):
    return ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF, round(a * 255))


def icon(size):
    """The app icon at `size` px, exactly as gfx::app_icon lays it out."""
    n = size * SS
    s = n / 32.0
    base = Image.new("RGBA", (n, n), (0, 0, 0, 0))

    def rrect(x, y, w, h, r, c, a=1.0):
        layer = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        ImageDraw.Draw(layer).rounded_rectangle(
            (x * s, y * s, (x + w) * s - 1, (y + h) * s - 1), radius=r * s, fill=rgb(c, a)
        )
        base.alpha_composite(layer)

    rrect(3, 3, 26, 26, 6, 0xC99A1E)
    rrect(4, 4, 24, 24, 5, 0xF7CF4E)
    for i in range(3):
        rrect(8, 10 + i * 5, 10 if i == 2 else 16, 2.4, 1.2, 0x6B5208, 0.85)
    return base.resize((size, size), Image.LANCZOS)


def tile(w, h, art):
    """A transparent `w`x`h` canvas with the icon (`art` px) centred."""
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    img.alpha_composite(icon(art), ((w - art) // 2, (h - art) // 2))
    return img


def main():
    OUT.mkdir(exist_ok=True)
    out = {}
    for scale in (100, 200):
        k = scale / 100
        out[f"Square44x44Logo.scale-{scale}.png"] = icon(round(44 * k))
        out[f"StoreLogo.scale-{scale}.png"] = icon(round(50 * k))
        # Tiles: the icon art covers about two thirds of the tile.
        out[f"Square150x150Logo.scale-{scale}.png"] = tile(round(150 * k), round(150 * k), round(100 * k))
    # Taskbar, Start list, Explorer: exact pixel sizes, plated and unplated.
    for t in (16, 24, 32, 48, 256):
        out[f"Square44x44Logo.targetsize-{t}.png"] = icon(t)
        out[f"Square44x44Logo.targetsize-{t}_altform-unplated.png"] = icon(t)
    for name, img in out.items():
        img.save(OUT / name, optimize=True)
        print(name)


if __name__ == "__main__":
    main()
