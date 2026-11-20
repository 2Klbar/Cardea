#!/usr/bin/env python3
"""Generates the Cardea logo ("keyhole arch"): theme variants, app icon, tray state icons, and a preview sheet."""
import pathlib

HERE = pathlib.Path(__file__).resolve().parent

THEMES = {  # "light" = for light backgrounds, "dark" = for dark backgrounds
    "light": {"ink": "#1d2b33", "accent": "#17867a", "ok": "#2e9e5b", "busy": "#2f6fd0", "err": "#d0453a",
              "bg": "#f6f8f9", "fg": "#1d2b33"},
    "dark": {"ink": "#e4ecee", "accent": "#5fd0c1", "ok": "#4cc27e", "busy": "#5b9cf0", "err": "#f06a5f",
             "bg": "#15191c", "fg": "#e4ecee"},
}

ARCH = "M24 108V58a40 40 0 0 1 80 0v50z"
KEYHOLE = '<circle cx="64" cy="62" r="13" fill="{c}"/><path d="M58 68h12l5 30h-22z" fill="{c}"/>'
STEP = '<rect x="14" y="112" width="100" height="9" rx="4.5" fill="{c}"/>'


def svg(body, size=128):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="{size}" height="{size}">\n'
            f'{body}\n</svg>\n')


def glyph(t):
    return f'<path d="{ARCH}" fill="{t["ink"]}"/>' + KEYHOLE.format(c=t["accent"]) + STEP.format(c=t["ink"])


def app_icon():
    # One icon for menus and window lists, readable on light and dark: a dark tile with the light-theme glyph inverted.
    d = THEMES["dark"]
    inner = glyph(d).replace("<path", "<path", 1)
    return (f'<rect x="4" y="4" width="120" height="120" rx="26" fill="#1d2b33"/>'
            f'<g transform="translate(64 66) scale(.74) translate(-64 -66)">{inner}</g>')


BADGE_MASK = '''<mask id="cut"><rect width="128" height="128" fill="#fff"/><circle cx="100" cy="100" r="33" fill="#000"/></mask>'''


def tray(t, state):
    if state == "idle":  # nothing running: outlined, dimmed glyph, no badge
        return (f'<g opacity=".6"><path d="{ARCH}" fill="none" stroke="{t["ink"]}" stroke-width="9" '
                f'stroke-linejoin="round"/>' + KEYHOLE.format(c=t["ink"]) + STEP.format(c=t["ink"]) + '</g>')
    color = {"running": t["ok"], "busy": t["busy"], "error": t["err"]}[state]
    mark = {
        "running": '<path d="M88 100l8 8 16-16" fill="none" stroke="#fff" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"/>',
        "busy": '<path d="M100 87A13 13 0 1 1 87 100" fill="none" stroke="#fff" stroke-width="6.5" stroke-linecap="round"/>'
                '<polygon points="80.5,101 93.5,101 87,90.5" fill="#fff"/>',
        "error": '<rect x="96" y="83" width="8" height="21" rx="4" fill="#fff"/><circle cx="100" cy="113" r="4.5" fill="#fff"/>',
    }[state]
    return (BADGE_MASK + f'<g mask="url(#cut)">{glyph(t)}</g>'
            f'<circle cx="100" cy="100" r="27" fill="{color}"/>{mark}')


files = {}
for name, t in THEMES.items():
    files[f"cardea-{name}.svg"] = svg(glyph(t))
    for state in ("idle", "running", "busy", "error"):
        files[f"tray/cardea-tray-{state}-{name}.svg"] = svg(tray(t, state), 22)
files["cardea-app.svg"] = svg(app_icon())

(HERE / "tray").mkdir(exist_ok=True)
for rel, content in files.items():
    (HERE / rel).write_text(content)

# Preview sheet
rows = []
for name, t in THEMES.items():
    tray_cells = "".join(
        f'<figure><img src="tray/cardea-tray-{s}-{name}.svg" width="44"><img src="tray/cardea-tray-{s}-{name}.svg" width="22">'
        f'<figcaption>{label}</figcaption></figure>'
        for s, label in (("idle", "No sandbox running"), ("running", "At least one running"),
                         ("busy", "Task in progress"), ("error", "Error")))
    sizes = "".join(f'<img src="cardea-{name}.svg" width="{n}">' for n in (64, 32, 22, 16))
    rows.append(f'''<div class="panel" style="background:{t['bg']};color:{t['fg']}">
<div class="lock"><img src="cardea-{name}.svg" width="96"><span>cardea</span></div>
<div class="sizes">{sizes}<img src="cardea-app.svg" width="64" title="app icon"><img src="cardea-app.svg" width="32"></div>
<div class="tray">{tray_cells}</div></div>''')
(HERE / "preview.html").write_text(f'''<!doctype html><meta charset="utf-8"><title>Cardea logo</title>
<style>body{{margin:0;padding:20px;width:1000px;font-family:"Noto Sans","DejaVu Sans",sans-serif;background:#e9edef}}
h1{{font-size:20px;margin:0 0 12px}} .panel{{border-radius:10px;padding:20px 24px;margin-bottom:14px}}
.lock{{display:flex;align-items:center;gap:18px;font-size:56px;font-weight:600;letter-spacing:1px}}
.sizes{{display:flex;align-items:flex-end;gap:20px;margin:18px 0}} .tray{{display:flex;gap:34px}}
figure{{margin:0;display:flex;align-items:flex-end;gap:10px}} figcaption{{font-size:13px;opacity:.8;width:90px}}</style>
<h1>Cardea — keyhole arch: logo, app icon (dark tile), tray states</h1>{''.join(rows)}''')
print("\n".join(sorted(files)))
