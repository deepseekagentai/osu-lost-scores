import struct
import subprocess
import sys
from pathlib import Path

PNG_SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
SMALL_MAX = 32
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICNS_TYPES = (
    (b"icp4", 16),
    (b"ic11", 32),
    (b"icp5", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
)


def render(svg, small, icon_dir):
    icon_dir.mkdir(parents=True, exist_ok=True)
    for size in PNG_SIZES:
        target = icon_dir / f"lemon-{size}.png"
        source = small if size <= SMALL_MAX else svg
        subprocess.run(
            ["resvg", "-w", str(size), "-h", str(size), str(source), str(target)],
            check=True,
        )


def build_ico(icon_dir, output):
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


def build_icns(icon_dir, output):
    entries = b""
    for kind, size in ICNS_TYPES:
        data = (icon_dir / f"lemon-{size}.png").read_bytes()
        entries += kind + struct.pack(">I", 8 + len(data)) + data
    output.write_bytes(b"icns" + struct.pack(">I", 8 + len(entries)) + entries)


if __name__ == "__main__":
    svg = Path(sys.argv[1])
    folder = Path(sys.argv[2])
    small = Path(sys.argv[3]) if len(sys.argv) > 3 else svg
    render(svg, small, folder)
    build_ico(folder, folder / "lemon.ico")
    build_icns(folder, folder / "lemon.icns")
