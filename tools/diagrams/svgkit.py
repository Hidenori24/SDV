"""Tiny helpers to write readable SVG diagrams (no dependencies)."""
from xml.sax.saxutils import escape

FONT = '"Hiragino Sans","Noto Sans CJK JP","Noto Sans JP","Yu Gothic","Meiryo",sans-serif'

# Colour roles (light background; every figure carries its own white backdrop so it also reads on dark pages)
C = {
    "ink": "#1f2937", "sub": "#6b7280", "line": "#9ca3af", "bg": "#ffffff", "panel": "#f8fafc",
    "swc": ("#e8f0fe", "#4a6fa5"), "model": ("#f3e8ff", "#8b5cf6"), "rte": ("#fff4e0", "#c28a2c"),
    "bsw": ("#e6f4ea", "#3d8b5a"), "io": ("#f1f5f9", "#64748b"), "ecu": ("#eef2ff", "#4f46e5"),
    "plant": ("#fff7ed", "#ea580c"), "bus": ("#dcfce7", "#16a34a"), "warn": ("#fef2f2", "#dc2626"),
    "cpp": ("#e0f2fe", "#0369a1"), "rust": ("#ffedd5", "#c2410c"), "gold": ("#fef9c3", "#ca8a04"),
}


def tw(s, size):
    """Rough text width: full-width for CJK, ~0.58em for the rest."""
    return sum(size if ord(ch) > 0x2E80 else size * 0.58 for ch in s)


class Svg:
    def __init__(self, w, h, title, desc):
        self.w, self.h = w, h
        self.parts = []
        self.title, self.desc = title, desc

    def add(self, s):
        self.parts.append(s)

    def rect(self, x, y, w, h, fill="#fff", stroke="#999", sw=1.5, rx=8, dash=None, extra=""):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d} {extra}/>')

    def text(self, x, y, s, size=14, weight="400", anchor="start", fill=None, extra=""):
        fill = fill or C["ink"]
        self.add(f'<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}" {extra}>{escape(s)}</text>')

    def lines(self, x, y, items, size=13, lh=None, anchor="start", fill=None, weight="400"):
        lh = lh or size * 1.5
        for i, s in enumerate(items):
            self.text(x, y + i * lh, s, size, weight, anchor, fill)
        return y + len(items) * lh

    def path(self, d, stroke="#555", sw=2, fill="none", dash=None, marker_end=None, marker_start=None):
        extra = ""
        if dash:
            extra += f' stroke-dasharray="{dash}"'
        if marker_end:
            extra += f' marker-end="url(#{marker_end})"'
        if marker_start:
            extra += f' marker-start="url(#{marker_start})"'
        self.add(f'<path d="{d}" stroke="{stroke}" stroke-width="{sw}" fill="{fill}" stroke-linejoin="round"{extra}/>')

    def label(self, x, y, s, size=12, fill=None, anchor="middle", pad=4, bg="#ffffff", weight="400"):
        w = tw(s, size) + pad * 2
        bx = x - w / 2 if anchor == "middle" else (x - pad if anchor == "start" else x - w + pad)
        self.add(f'<rect x="{bx:.1f}" y="{y - size:.1f}" width="{w:.1f}" height="{size + 6}" rx="3" fill="{bg}" fill-opacity="0.92"/>')
        self.text(x, y, s, size, weight, anchor, fill)

    def vlabel(self, x, y, s, size=12, fill=None, pad=4, bg="#ffffff", weight="400"):
        """Label rotated by -90 degrees, centred on (x, y) - for text next to vertical lines."""
        w = tw(s, size) + pad * 2
        self.add(f'<g transform="rotate(-90 {x} {y})">'
                 f'<rect x="{x - w / 2:.1f}" y="{y - size:.1f}" width="{w:.1f}" height="{size + 6}" rx="3" fill="{bg}" fill-opacity="0.92"/>')
        self.text(x, y, s, size, weight, "middle", fill)
        self.add('</g>')

    def badge(self, x, y, n, color="#dc2626"):
        self.add(f'<circle cx="{x}" cy="{y}" r="11" fill="{color}"/>')
        self.text(x, y + 4.5, str(n), 13, "700", "middle", "#fff")

    def render(self):
        markers = ""
        for name, color in [("a_blue", "#2563eb"), ("a_orange", "#d97706"), ("a_dark", "#374151"), ("a_red", "#dc2626"), ("a_green", "#15803d"), ("a_gray", "#6b7280")]:
            markers += (f'<marker id="{name}" viewBox="0 0 10 10" refX="9" refY="5" markerUnits="userSpaceOnUse" markerWidth="13" markerHeight="13" orient="auto-start-reverse">'
                        f'<path d="M0,0 L10,5 L0,10 z" fill="{color}"/></marker>')
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" width="{self.w}" height="{self.h}" '
                f'font-family=\'{FONT}\' role="img" aria-labelledby="t d">'
                f'<title id="t">{escape(self.title)}</title><desc id="d">{escape(self.desc)}</desc>'
                f'<defs>{markers}</defs>'
                f'<rect x="0.5" y="0.5" width="{self.w - 1}" height="{self.h - 1}" rx="12" fill="{C["bg"]}" stroke="#d1d5db"/>'
                + "".join(self.parts) + "</svg>\n")
