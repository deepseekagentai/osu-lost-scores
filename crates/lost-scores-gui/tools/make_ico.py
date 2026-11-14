import struct
import subprocess
import sys
from pathlib import Path

PNG_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


def render(svg, icon_dir):
    icon_dir.mkdir(parents=True, exist_ok=True)
    for size in PNG_SIZES:
        target = icon_dir / f"lemon-{size}.png"
        subprocess.run(
            ["resvg", "-w", str(size), "-h", str(size), str(svg), str(target)],
            check=True,
        )


def build(icon_dir, output):
    images = [(size, (icon_dir / f"lemon-{size}.png").read_bytes()) for size in ICO_SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries = b""
    payload = b""
    for size, data in images:
        side = 0 if size >= 256 else size
        entries += struct.pack(
            "<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset + len(payload)
        )
        payload += data
    output.write_bytes(header + entries + payload)


if __name__ == "__main__":
    svg = Path(sys.argv[1])
    folder = Path(sys.argv[2])
    render(svg, folder)
    build(folder, folder / "lemon.ico")
