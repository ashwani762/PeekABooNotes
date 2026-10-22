# Copyright (C) 2026 Ashwani Kumar
# SPDX-License-Identifier: GPL-3.0-or-later

"""Bundle the MSIX logos (the same drawing as the app's in-code icon) into app.ico, for
the setup file, the shortcuts and Apps & features. Needs Pillow only. Run:
python packaging/inno/make-icon.py"""

from pathlib import Path

from PIL import Image

here = Path(__file__).parent
assets = here.parent / 'msix' / 'Assets'
sizes = [16, 24, 32, 48, 256]
images = [Image.open(assets / f'Square44x44Logo.targetsize-{n}.png').convert('RGBA') for n in sizes]
# Each size is its own drawing (crisp at 16 px), not the 256 px one scaled down.
images[-1].save(here / 'app.ico', format='ICO', sizes=[(n, n) for n in sizes], append_images=images[:-1])
print('wrote', here / 'app.ico')
