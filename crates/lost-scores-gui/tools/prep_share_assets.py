import atexit
import os
import re
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw

CRATE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
A = f"{CRATE}/assets"
S = f"{A}/share"
SS = 4
BG = (0x1D, 0x1C, 0x22)
GRADES = {
    "XH": "SS-Silver", "SH": "S-Silver", "X": "SS", "S": "S",
    "A": "A", "B": "B", "C": "C", "D": "D",
}
MODS = {
    "HD": ("hidden", "ff6666", "591e1e"), "HR": ("hard-rock", "ff6666", "591e1e"),
    "DT": ("double-time", "ff6666", "591e1e"), "NC": ("nightcore", "ff6666", "591e1e"),
    "FL": ("flashlight", "ff6666", "591e1e"), "SD": ("sudden-death", "ff6666", "591e1e"),
    "PF": ("perfect", "ff6666", "591e1e"), "EZ": ("easy", "b3ff66", "3c591e"),
    "NF": ("no-fail", "b3ff66", "3c591e"), "HT": ("half-time", "b3ff66", "3c591e"),
    "SO": ("spun-out", "66ccff", "1e4659"), "TD": ("touch-device", "ffcc22", "594605"),
}
WORK = tempfile.TemporaryDirectory(prefix="osu-lsa_svg_")
atexit.register(WORK.cleanup)
TMP = WORK.name


def render(svg, w, h, text=None):
    src = os.path.join(TMP, "in.svg")
    if text is not None:
        with open(src, "w", encoding="utf-8") as f:
            f.write(text)
        svg = src
    out = os.path.join(TMP, "out.png")
    subprocess.run(["resvg", "-w", str(w), "-h", str(h), svg, out], check=True)
    return Image.open(out).convert("RGBA")


def solid(hexcolor, mask):
    im = Image.new("RGBA", mask.size, tuple(int(hexcolor[i:i + 2], 16) for i in (0, 2, 4)) + (255,))
    im.putalpha(mask)
    return im


def rounded(w, h, r, top=True, bottom=True):
    big = Image.new("L", (w * SS, h * SS), 0)
    d = ImageDraw.Draw(big)
    d.rounded_rectangle((0, 0, w * SS - 1, h * SS - 1), radius=r * SS, fill=255)
    if not top:
        d.rectangle((0, 0, w * SS - 1, h * SS // 2), fill=255)
    if not bottom:
        d.rectangle((0, h * SS // 2, w * SS - 1, h * SS - 1), fill=255)
    return big.resize((w, h), Image.LANCZOS)


shutil.rmtree(S, ignore_errors=True)
for sub in ("grades", "mods"):
    os.makedirs(f"{S}/{sub}")

for name, file in GRADES.items():
    render(f"{A}/osu-web/GradeSmall-{file}.svg", 96, 48).save(f"{S}/grades/{name}.png")

shape = render(f"{A}/osu-web/mod-icon.svg", 60, 42).getchannel("A")
for acr, (name, bg, fg) in MODS.items():
    icon = solid(bg, shape)
    icon.alpha_composite(solid(fg, render(f"{A}/osu-web/mod-{name}.svg", 60, 42).getchannel("A")))
    icon.save(f"{S}/mods/{acr}.png")

for name, size in (("i-clock", 11), ("i-trend-down", 12)):
    mask = render(f"{A}/{name}.svg", size * 2, size * 2).getchannel("A")
    solid("ffffff", mask).save(f"{S}/{name[2:]}.png")
for size in (18, 24):
    render(f"{A}/lemon.svg", size * 2, size * 2).save(f"{S}/lemon-{size * 2}.png")

tri = open(f"{A}/tri_top.svg", encoding="utf-8").read()
head = re.match(r"<svg[^>]*>", tri).group(0)


def triangles(view, w, h):
    tag = re.sub(r"viewBox='[^']*'", f"viewBox='{view}'", head)
    tag = re.sub(r"width='[^']*'", f"width='{w}'", tag)
    tag = re.sub(r"height='[^']*'", f"height='{h}'", tag)
    return render(None, w, h, tri.replace(head, tag, 1))


def backdrop(tri_layer, w, h, bottom_round):
    im = Image.new("RGBA", (w, h), BG + (255,))
    im.alpha_composite(tri_layer.crop((0, 0, w, h)))
    over = Image.new("RGBA", (w, h), BG + (0,))
    a = Image.new("L", (w, h))
    ad = ImageDraw.Draw(a)
    for y in range(h):
        ad.line([(0, y), (w, y)], fill=round(255 * min(1.0, max(0.0, (y / 880 - 0.1) / 0.9))))
    over.putalpha(a)
    im.alpha_composite(over)
    im.putalpha(rounded(w, h, 36, top=True, bottom=bottom_round))
    return im


scale = 980 / 650
visible = 440 / scale
backdrop(triangles(f"0 {(440 - visible) / 2:.4f} 650 {visible:.4f}", 1960, 880), 1960, 880, False).save(f"{S}/backdrop-wide.png")
backdrop(triangles("65 0 520 440", 1040, 880), 1040, 280, True).save(f"{S}/backdrop-badge.png")
for root, _, files in sorted(os.walk(S)):
    for file in sorted(files):
        path = os.path.join(root, file)
        print(os.path.relpath(path, S), Image.open(path).size)
