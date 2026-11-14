import math
import sys

from fontTools.ttLib import TTFont

ANGLE = 10.0


def slant(regular_path, italic_path):
    font = TTFont(regular_path)
    factor = math.tan(math.radians(ANGLE))
    glyf = font["glyf"]
    for name in font.getGlyphOrder():
        glyph = glyf[name]
        if glyph.isComposite():
            for component in glyph.components:
                component.x = round(component.x + component.y * factor)
        elif glyph.numberOfContours > 0:
            coordinates = glyph.coordinates
            for index, (x, y) in enumerate(coordinates):
                coordinates[index] = (round(x + y * factor), y)
    for name in font.getGlyphOrder():
        glyph = glyf[name]
        glyph.recalcBounds(glyf)
        advance, _ = font["hmtx"][name]
        font["hmtx"][name] = (advance, getattr(glyph, "xMin", 0))
    font["post"].italicAngle = -ANGLE
    font["head"].macStyle = font["head"].macStyle | 2
    selection = font["OS/2"].fsSelection
    font["OS/2"].fsSelection = (selection & ~0x40) | 0x01
    names = font["name"]
    for record in names.names:
        if record.nameID == 2:
            record.string = "Italic"
        elif record.nameID == 4:
            record.string = "Fredoka Italic"
        elif record.nameID == 6:
            record.string = "Fredoka-Italic"
    font.save(italic_path)


if __name__ == "__main__":
    slant(sys.argv[1], sys.argv[2])
