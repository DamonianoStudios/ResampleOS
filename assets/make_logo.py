"""Generates the ResampleOS logo and icon files from one description of the mark.

    python assets/make_logo.py

Writes, next to this script:
    logo.svg        the mark as vector art
    banner.svg      the mark with the wordmark, for the README
    logo.png        1024 px, for avatars and social cards
    icon-256.png    embedded in the app as the window icon
    icon.ico        embedded in the Windows executable (16 to 256 px)

The mark is a 2x2 pad grid. The bottom-left pad (pad 1 on the hardware-style matrix) is
lit amber, the colour of audio in the app, and carries a loop-back arrow: the skip-back
and resample idea in one glyph. The top-right pad is cyan, the colour of MIDI. Small sizes
drop the arrow and the border so the grid stays legible at 16 px.

Requires Pillow.
"""

import math
import os

from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))

CHASSIS = (20, 22, 28)
EDGE = (48, 53, 66)
PAD = (44, 48, 60)
AMBER = (255, 176, 46)
CYAN = (64, 208, 228)

# Geometry on a 1024 canvas.
SIZE = 1024
CORNER = 224
GRID_INSET, GAP, PAD_RADIUS = 186, 60, 62
ARROW_RADIUS, ARROW_WIDTH = 74, 30
ARROW_START, ARROW_SWEEP = -100, 285  # degrees clockwise from 3 o'clock; the head sits at the start


def pads(inset, gap):
    """(x, y, size, colour, lit) for the four pads, top-left first."""
    size = (SIZE - 2 * inset - gap) / 2
    a, b = inset, inset + size + gap
    return [
        (a, a, size, PAD, False),
        (b, a, size, CYAN, True),
        (a, b, size, AMBER, True),
        (b, b, size, PAD, False),
    ]


def arrow_points(cx, cy):
    """The arrowhead triangle for the loop-back glyph: at the arc's start, pointing
    counter-clockwise, so the arrow reads as "go back"."""
    a = math.radians(ARROW_START)
    hx, hy = cx + ARROW_RADIUS * math.cos(a), cy + ARROW_RADIUS * math.sin(a)
    tx, ty = math.sin(a), -math.cos(a)
    nx, ny = math.cos(a), math.sin(a)
    head, half = 54, 42
    tip = (hx + tx * head, hy + ty * head)
    return [tip, (hx + nx * half, hy + ny * half), (hx - nx * half, hy - ny * half)]


def render(px, detailed=True):
    """Draws the mark at `px` pixels, supersampled for clean edges."""
    scale = 4 if px <= 256 else 2
    n = px * scale
    k = n / SIZE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, n - 1, n - 1], radius=CORNER * k, fill=CHASSIS)
    if detailed:
        d.rounded_rectangle([6 * k, 6 * k, n - 1 - 6 * k, n - 1 - 6 * k], radius=(CORNER - 6) * k, outline=EDGE, width=max(1, round(10 * k)))

    inset, gap = (GRID_INSET, GAP) if detailed else (150, 84)
    cells = pads(inset, gap)

    # Soft light spilling from the lit pads.
    glow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    for x, y, s, col, lit in cells:
        if lit:
            gd.rounded_rectangle([x * k, y * k, (x + s) * k, (y + s) * k], radius=PAD_RADIUS * k, fill=col + (150,))
    glow = glow.filter(ImageFilter.GaussianBlur(34 * k))
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, n - 1, n - 1], radius=CORNER * k, fill=255)
    img.paste(Image.alpha_composite(img, glow), (0, 0), mask)
    d = ImageDraw.Draw(img)

    for x, y, s, col, lit in cells:
        box = [x * k, y * k, (x + s) * k, (y + s) * k]
        d.rounded_rectangle(box, radius=PAD_RADIUS * k, fill=col)

    if detailed:
        x, y, s, _, _ = cells[2]
        cx, cy = (x + s / 2) * k, (y + s / 2) * k
        r, w = ARROW_RADIUS * k, ARROW_WIDTH * k
        d.arc([cx - r - w / 2, cy - r - w / 2, cx + r + w / 2, cy + r + w / 2], ARROW_START, ARROW_START + ARROW_SWEEP, fill=CHASSIS, width=round(w))
        tri = arrow_points(x + s / 2, y + s / 2)
        d.polygon([(px_ * k, py_ * k) for px_, py_ in tri], fill=CHASSIS)
    return img.resize((px, px), Image.LANCZOS)


def hexcol(c):
    return "#%02x%02x%02x" % c


def mark_svg():
    """The detailed mark as SVG elements on a 1024 canvas."""
    out = [
        '<defs><filter id="glow" x="-40%" y="-40%" width="180%" height="180%"><feGaussianBlur stdDeviation="34"/></filter>',
        f'<clipPath id="chassis"><rect width="{SIZE}" height="{SIZE}" rx="{CORNER}"/></clipPath></defs>',
        f'<rect width="{SIZE}" height="{SIZE}" rx="{CORNER}" fill="{hexcol(CHASSIS)}"/>',
        f'<rect x="11" y="11" width="{SIZE - 22}" height="{SIZE - 22}" rx="{CORNER - 11}" fill="none" stroke="{hexcol(EDGE)}" stroke-width="10"/>',
        '<g clip-path="url(#chassis)" filter="url(#glow)" opacity="0.6">',
    ]
    cells = pads(GRID_INSET, GAP)
    for x, y, s, col, lit in cells:
        if lit:
            out.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{s:.1f}" height="{s:.1f}" rx="{PAD_RADIUS}" fill="{hexcol(col)}"/>')
    out.append("</g>")
    for x, y, s, col, lit in cells:
        out.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{s:.1f}" height="{s:.1f}" rx="{PAD_RADIUS}" fill="{hexcol(col)}"/>')
    x, y, s, _, _ = cells[2]
    cx, cy = x + s / 2, y + s / 2
    a0 = math.radians(ARROW_START)
    sx, sy = cx + ARROW_RADIUS * math.cos(a0), cy + ARROW_RADIUS * math.sin(a0)
    a1 = math.radians(ARROW_START + ARROW_SWEEP)
    ex, ey = cx + ARROW_RADIUS * math.cos(a1), cy + ARROW_RADIUS * math.sin(a1)
    tri = arrow_points(cx, cy)
    out.append(
        f'<path d="M {sx:.1f} {sy:.1f} A {ARROW_RADIUS} {ARROW_RADIUS} 0 1 1 {ex:.1f} {ey:.1f}" fill="none" '
        f'stroke="{hexcol(CHASSIS)}" stroke-width="{ARROW_WIDTH}"/>'
    )
    out.append('<polygon points="%s" fill="%s"/>' % (" ".join(f"{px:.1f},{py:.1f}" for px, py in tri), hexcol(CHASSIS)))
    return "\n  ".join(out)


def write(name, text):
    with open(os.path.join(HERE, name), "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def main():
    write("logo.svg", f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}" role="img" aria-label="ResampleOS">\n  {mark_svg()}\n</svg>\n')

    # The wordmark is live text in a system font stack, so no font is redistributed. The
    # banner has its own dark plate and reads the same on light and dark pages.
    w, h, m = 1280, 360, 60
    s = (h - 2 * m) / SIZE
    write(
        "banner.svg",
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" role="img" aria-label="ResampleOS: resample-first pad DAW">\n'
        f'  <rect width="{w}" height="{h}" rx="36" fill="#0a0b0e"/>\n'
        f'  <g transform="translate({m} {m}) scale({s:.5f})">\n  {mark_svg()}\n  </g>\n'
        f'  <g font-family="Inter, \'Segoe UI\', system-ui, -apple-system, Helvetica, Arial, sans-serif">\n'
        f'    <text x="{m + 290}" y="176" font-size="104" font-weight="700" letter-spacing="-2" fill="#eceef3">Resample<tspan fill="{hexcol(AMBER)}">OS</tspan></text>\n'
        f'    <text x="{m + 294}" y="236" font-size="34" font-weight="400" fill="#868d9e">Commit to audio. Capture everything. Resample it again.</text>\n'
        f"  </g>\n</svg>\n",
    )

    render(1024).save(os.path.join(HERE, "logo.png"))
    big = render(256)
    big.save(os.path.join(HERE, "icon-256.png"))
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [render(px, detailed=px >= 48) for px in sizes]
    frames[-1].save(os.path.join(HERE, "icon.ico"), format="ICO", sizes=[(px, px) for px in sizes], append_images=frames[:-1])
    print("wrote logo.svg, banner.svg, logo.png, icon-256.png, icon.ico")


if __name__ == "__main__":
    main()
