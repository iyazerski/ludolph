# dmgbuild settings for Ludolph.dmg: the app beside a link to Applications, with
# an arrow between them. Run from the repo root:
#   uvx dmgbuild -s scripts/dmg.py -D app=<Ludolph.app> Ludolph <out.dmg>
# ruff: noqa: F821 (dmgbuild runs this file with `defines` in scope)
import os

app = defines["app"]

format = "UDZO"
filesystem = "APFS"
files = [app]
symlinks = {"Applications": "/Applications"}
# The volume's icon in Finder.
icon = "assets/Ludolph.icns"

# Drawn from assets/dmg-background.svg at 1x and 2x, joined with `tiffutil -cathidpicheck`.
background = "assets/dmg-background.tiff"
window_rect = ((200, 120), (540, 360))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 112
text_size = 13
hide_extensions = [os.path.basename(app)]
icon_locations = {
    os.path.basename(app): (140, 170),
    "Applications": (400, 170),
}
