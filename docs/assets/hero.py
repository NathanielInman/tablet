#!/usr/bin/env python3
"""Writes docs/assets/hero.svg: the README's animated hero.

An 18 second loop on the same desktop as Roundhouse's, Switchyard's,
Tannoy's and Timetable's heroes, under the floating Platform bar. An agent
in a terminal runs `tablet need work` and the Tablet lamp turns amber; a
right-click opens the drawer with the request; Connect starts openvpn3,
the web sign-in finishes in the browser, the lamp goes frost and the build
runs. Then the server goes quiet: the probes fail, the lamp and the drawer
turn orange (stale), Reconnect brings it back, and the Checks and History
tabs show the probes, the tunnel's DNS and what happened. A click outside
closes the drawer.

The drawer is drawn by `draw_drawer()` in its own coordinates at its real
size (400 px wide, as `drawer.width` defaults), one state at a time, so the
desktop tour in dot-files can port it. The story stacks one drawer per
state and swaps them.

Pure CSS animation inside the SVG, so GitHub plays it in an <img>. With
reduced motion it is a still of the open drawer holding the tunnel.

    python3 docs/assets/hero.py
"""

import textwrap
from dataclasses import dataclass, field
from pathlib import Path

W, H = 1200, 660
T = 18  # loop seconds

# Nord, matching the Platform waybar and the siblings' heroes.
BG0, BG1 = "#14171c", "#0d0f12"
PILL, PILL_LINE, PILL_ON = "#24262a", "#3a404d", "#3c4651"
BAR_FG, BRIGHT, BAR_DIM = "#c5c8c6", "#d8dee9", "#616e88"
PANEL, PANEL_LINE, WELL = "#1e2024", "#41464e", "#15171a"
TILE, TILE_LINE, HOVER = "#25282c", "#2f3237", "#33373d"
FG, DIM, IDLE = "#c5c8c6", "#7c838e", "#555b64"
FROST, GO, AMBER, ORANGE, RED, PURPLE = "#88c0d0", "#a3be8c", "#ebcb8b", "#d08770", "#bf616a", "#b48ead"
FONT = "JetBrains Mono, DejaVu Sans Mono, Menlo, Consolas, monospace"

# The README's alt text says the same.
ALT = (
    "A floating waybar of frosted pills: workspaces, the window title, Tannoy's song, then the lamps, where "
    "Tablet's network lamp is dim. In a terminal an agent runs tablet need work for a Gradle build, and the lamp "
    "turns amber with the word asked. A right-click opens the Tablet drawer out of it: the agent's request with "
    "its command, directory and unverified reason, and Connect work. A click on Connect turns the lamp into a "
    "pulsing ellipsis, then sign in while a browser finishes the web sign-in, then frost: the drawer holds work "
    "with its traffic graph and both probes answering, and the build runs in the terminal. The server goes quiet: "
    "the probes fail twice, the lamp says stale in orange and the drawer explains that openvpn3 says connected "
    "but nothing answers. A click on Reconnect brings it back. The Checks tab shows both probes, the resolvers "
    "and routing-only zones on tun0 and openvpn3's counters; the History tab shows connected, stale, reconnected "
    "and an older dropped session. A click outside closes the drawer."
)


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def tx(x, y, s, size=13, fill=FG, weight=400, anchor="start", spacing=0, attrs=""):
    """A <text> element as a string."""
    ls = f' letter-spacing="{spacing}"' if spacing else ""
    attrs = f" {attrs}" if attrs else ""
    return f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}" font-weight="{weight}" text-anchor="{anchor}"{ls}{attrs}>{esc(s)}</text>'


def text_w(s, size):
    """A monospace advance: 0.6em a character."""
    return len(s) * size * 0.6


# ---------------------------------------------------------------- glyphs
# Drawn, not typed: the Nerd Font glyphs would not survive an <img>.

def g_lan(cx, cy, c, connected=True):
    """md-lan-connect / md-lan-disconnect: a box above two, joined, or with
    the joint cut."""
    o = (f'<g fill="none" stroke="{c}" stroke-width="1.4"><rect x="{cx - 2.5}" y="{cy - 7}" width="5" height="4" rx="1"/>'
         f'<rect x="{cx - 7}" y="{cy + 3}" width="5" height="4" rx="1"/><rect x="{cx + 2}" y="{cy + 3}" width="5" height="4" rx="1"/>'
         f'<path d="M{cx - 4.5},{cy + 3} v-2 h9 v2"/>')
    o += f'<path d="M{cx},{cy - 3} v2"/>' if connected else f'<path d="M{cx - 2},{cy - 2.6} l4,1.6" stroke-width="1.2"/>'
    return o + "</g>"


def g_bell(cx, cy, c):
    return (f'<path d="M{cx - 5.5},{cy + 3.5} L{cx - 5.5},{cy - 1} Q{cx - 5.5},{cy - 6.5} {cx},{cy - 6.5} Q{cx + 5.5},{cy - 6.5} {cx + 5.5},{cy - 1} L{cx + 5.5},{cy + 3.5} L{cx + 7},{cy + 5} L{cx - 7},{cy + 5} Z" fill="{c}"/>'
            f'<circle cx="{cx}" cy="{cy + 7}" r="1.8" fill="{c}"/>')


def g_speaker(cx, cy, c):
    return (f'<path d="M{cx - 7},{cy - 2.5} L{cx - 4},{cy - 2.5} L{cx},{cy - 6} L{cx},{cy + 6} L{cx - 4},{cy + 2.5} L{cx - 7},{cy + 2.5} Z" fill="{c}"/>'
            f'<path d="M{cx + 3},{cy - 3} Q{cx + 5.5},{cy} {cx + 3},{cy + 3} M{cx + 5.5},{cy - 5.5} Q{cx + 10},{cy} {cx + 5.5},{cy + 5.5}" fill="none" stroke="{c}" stroke-width="1.5" stroke-linecap="round"/>')


def g_check(cx, cy, c, s=1.0):
    return f'<path d="M{cx - 4 * s},{cy} l{3 * s},{3 * s} l{5.5 * s},{-6.5 * s}" fill="none" stroke="{c}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>'


def g_cross(cx, cy, c, s=1.0):
    return f'<path d="M{cx - 3.5 * s},{cy - 3.5 * s} l{7 * s},{7 * s} M{cx + 3.5 * s},{cy - 3.5 * s} l{-7 * s},{7 * s}" stroke="{c}" stroke-width="1.8" stroke-linecap="round"/>'


def g_dot(cx, cy, c):
    return f'<circle cx="{cx}" cy="{cy}" r="2.4" fill="{c}"/>'


# ---------------------------------------------------------------- the drawer
# crates/tablet-board: drawer.rs builds it, style.css sizes it. Everything
# below is in the drawer's own coordinates, (0, 0) at the panel's top left.

# What each lamp state looks like: (bar color, word beside the glyph, the
# drawer chip's text). bar::lamp and the board's style.css.
LAMPS = {
    "off": (BAR_DIM, "", "off"),
    "asking": (AMBER, "asked", "asked"),
    "connecting": (AMBER, "…", "connecting"),
    "signin": (AMBER, "sign in", "sign in"),
    "held": (FROST, "", "held"),
    "stale": (ORANGE, "stale", "stale"),
    "dropped": (RED, "dropped", "dropped"),
}
PHASE = {  # a profile's phase: (color, label)
    "off": (IDLE, "off"),
    "connecting": (AMBER, "connecting"),
    "signin": (AMBER, "sign in"),
    "held": (FROST, "held"),
    "stale": (ORANGE, "stale"),
}


@dataclass(frozen=True)
class Probe:
    target: str
    ok: bool
    right: str


PROBES_OK = (Probe("git.corp.example.com:22", True, "31 ms"), Probe("wiki.corp.example.com:443", True, "44 ms"))
PROBES_BAD = (Probe("git.corp.example.com:22", False, "timed out"), Probe("wiki.corp.example.com:443", False, "timed out"))
HISTORY = (
    ("14:32", "work reconnected", FROST, "vpn.example.com"),
    ("14:31", "work stale", ORANGE, "openvpn3 says connected, but all 2 probes failed twice"),
    ("14:28", "work connected", FROST, "vpn.example.com"),
    ("Thu 18:20", "work disconnected", IDLE, ""),
    ("Thu 11:40", "work reconnecting", AMBER, "lost the server, reconnecting"),
    ("Wed 07:14", "work failed", RED, "the server refused the sign-in (AUTH_FAILED)"),
    ("Nov 4", "openvpn3 repaired", FROST, "openvpn3 is fine (8 binaries link)"),
    ("Nov 4", "openvpn3 broken", RED, "libprotobuf.so.36.0.0 is missing"),
)


@dataclass(frozen=True)
class Board:
    lamp: str = "held"  # the header's lamp and chip: a LAMPS key
    tab: str = "now"  # now, checks, doctor, history
    phase: str = "held"  # the work profile's phase: a PHASE key
    ask: bool = False  # an agent's request waits above the tabs
    big: str = "2m"  # the card's big line
    reason: str = ""  # the card's reason, in the phase's color
    probes: tuple = PROBES_OK
    graph: tuple = field(default=(3, 5, 4, 9, 14, 11, 22, 18, 12, 16, 26, 30, 21, 17, 24, 28, 33, 27, 19, 23))


DRAWER_W = 400  # drawer.width
P = 15  # the panel's border and padding
IW = DRAWER_W - 2 * P
CP = 11  # a card's padding
CW_ = IW - 2 * CP  # a card's inner width
GAP = 12  # between the panel's rows
LINE = 6  # between a card's rows
HEADER_H, TABS_H, FOOT_H, BTN_H = 30, 26, 13, 26
REASON_COLS = 46


def _card(o, y, h, stroke=TILE_LINE, fill=TILE, fill_op=1.0):
    o.append(f'<rect x="{P}" y="{y}" width="{IW}" height="{h}" rx="10" fill="{fill}" fill-opacity="{fill_op}" stroke="{stroke}"/>')


def _label(x, y, s, fill=DIM, anchor="start"):
    return tx(x, y, s, 10, fill, 700, anchor, 1)


def _buttons(o, spots, x, y, names, primary_color=FROST):
    """A row of buttons; the first one named in `primary` is filled."""
    for name, text, primary in names:
        w = text_w(text, 12) + 24
        if primary:
            o.append(f'<rect x="{x:.1f}" y="{y}" width="{w:.1f}" height="{BTN_H}" rx="8" fill="{primary_color}"/>')
            o.append(tx(x + w / 2, y + 17.5, text, 12, WELL, 700, "middle"))
        else:
            o.append(f'<rect x="{x:.1f}" y="{y + .5}" width="{w:.1f}" height="{BTN_H - 1}" rx="8" fill="{TILE}" stroke="{TILE_LINE}"/>')
            o.append(tx(x + w / 2, y + 17.5, text, 12, FG, 400, "middle"))
        spots[name] = (x, y, w, BTN_H)
        x += w + 6


def _ask_h():
    return CP + 14 + LINE + 18 + LINE + 16 + LINE + 16 + LINE + 2 * 16 + LINE + BTN_H + CP


def _ask(o, spots, y):
    h = _ask_h()
    _card(o, y, h, stroke=AMBER, fill=AMBER, fill_op=0.06)
    o.append(f'<rect x="{P}" y="{y}" width="{IW}" height="{h}" rx="10" fill="none" stroke="{AMBER}" stroke-opacity=".55"/>')
    x, cy = P + CP, y + CP + 10
    o.append(_label(x, cy, "AGENT ASKING"))
    o.append(_label(P + IW - CP, cy, "4s", anchor="end"))
    cy += 4 + LINE + 14
    o.append(tx(x, cy, "claude wants work", 14, FG, 700))
    cy += 4 + LINE + 12
    o.append(tx(x, cy, "$ ./gradlew assemble", 12, FG))
    cy += 4 + LINE + 12
    o.append(tx(x, cy, "in ~/Sites/storefront", 12, DIM))
    cy += 4 + LINE + 12
    for line in textwrap.wrap("“Nexus only answers on the VPN” (the agent's reason, unverified)", REASON_COLS):
        o.append(tx(x, cy, line, 12, DIM))
        cy += 16
    cy += LINE - 12
    _buttons(o, spots, x, cy, [("connect", "Connect work", True), ("notnow", "Not now", False)], AMBER)
    return h


def _tabs(o, spots, y, tab, alert=False):
    w = (IW - 3 * 6) / 4
    for i, (key, name) in enumerate([("now", "Now"), ("checks", "Checks"), ("doctor", "Doctor"), ("history", "History")]):
        x = P + i * (w + 6)
        on = key == tab
        if on:
            o.append(f'<rect x="{x:.1f}" y="{y}" width="{w:.1f}" height="{TABS_H}" rx="7" fill="{FROST}" fill-opacity=".75"/>')
        color = WELL if on else (RED if key == "doctor" and alert else DIM)
        o.append(tx(x + w / 2, y + 17.5, name, 11.5, color, 700 if on else 400, "middle"))
        spots[f"tab:{key}"] = (x, y, w, TABS_H)


def _now_h(b):
    h = CP + 14 + LINE + 30 + LINE + 16
    reason = textwrap.wrap(b.reason, REASON_COLS) if b.reason else []
    h += (LINE + 16 * len(reason)) if reason else 0
    if b.phase in ("held", "stale"):
        h += LINE + 38 + LINE + 16
    return h + LINE + BTN_H + CP


def _now(o, spots, y, b):
    color, label = PHASE[b.phase]
    h = _now_h(b)
    if b.phase == "stale":
        _card(o, y, h, stroke=ORANGE)
        o.append(f'<rect x="{P}" y="{y}" width="{IW}" height="{h}" rx="10" fill="{TILE}" stroke="{ORANGE}" stroke-opacity=".45"/>')
    elif b.phase in ("connecting", "signin"):
        _card(o, y, h)
        o.append(f'<rect x="{P}" y="{y}" width="{IW}" height="{h}" rx="10" fill="none" stroke="{AMBER}" stroke-opacity=".35"/>')
    else:
        _card(o, y, h)
    x, cy = P + CP, y + CP + 10
    o.append(_label(x, cy, "WORK · DEFAULT"))
    o.append(_label(P + IW - CP, cy, label, color, "end"))
    cy += 4 + LINE + 26
    o.append(tx(x, cy, b.big, 24, IDLE if b.phase == "off" else color, 700))
    cy += 4 + LINE + 12
    sub = {"off": "~/work.ovpn", "connecting": "", "signin": "", "held": "tun0 · udp · session from 14:28", "stale": "tun0 · udp · session from 14:28"}[b.phase]
    if b.phase in ("connecting", "signin"):
        sub = "vpn.example.com"
    o.append(tx(x, cy, sub, 12, DIM))
    cy += 4
    for line in textwrap.wrap(b.reason, REASON_COLS) if b.reason else []:
        cy += 16 if line != textwrap.wrap(b.reason, REASON_COLS)[0] else LINE + 12
        o.append(tx(x, cy, line, 12, color))
    if b.reason:
        cy += 4
    if b.phase in ("held", "stale"):
        # The last minute of traffic in, as a filled area, in the card's color.
        gy, gh = cy + LINE, 38
        pts = b.graph
        top = max(pts) * 1.15
        step = CW_ / (len(pts) - 1)
        line = " ".join(f"{x + i * step:.1f},{gy + gh - 1 - v / top * (gh - 3):.1f}" for i, v in enumerate(pts))
        gc = ORANGE if b.phase == "stale" else FROST
        o.append(f'<polygon points="{x:.1f},{gy + gh} {line} {x + CW_:.1f},{gy + gh}" fill="{gc}" fill-opacity=".2"/>')
        o.append(f'<polyline points="{line}" fill="none" stroke="{gc}" stroke-width="1.5"/>')
        cy = gy + gh + LINE + 12
        down, up = ("↓ 0 B/s", "↑ 1.2 kB/s") if b.phase == "stale" else ("↓ 1.4 MB/s", "↑ 86 kB/s")
        o.append(tx(x, cy, down, 12, FG))
        o.append(tx(x + text_w(down, 12) + 12, cy, up, 12, DIM))
        n = sum(p.ok for p in b.probes)
        o.append(tx(P + IW - CP, cy, f"{n}/{len(b.probes)} probes ok just now", 12, DIM, anchor="end"))
        cy += 4
    by = cy + LINE
    if b.phase == "off":
        _buttons(o, spots, x, by, [("card:connect", "Connect", True)])
    elif b.phase in ("connecting",):
        _buttons(o, spots, x, by, [("card:cancel", "Cancel", False)])
    elif b.phase == "signin":
        _buttons(o, spots, x, by, [("card:signin", "Open sign-in", True), ("card:cancel", "Cancel", False)])
    elif b.phase == "held":
        _buttons(o, spots, x, by, [("card:disconnect", "Disconnect", False), ("card:reconnect", "Reconnect", False), ("card:check", "Check now", False)])
    else:
        _buttons(o, spots, x, by, [("card:reconnect", "Reconnect", True), ("card:check", "Check now", False), ("card:disconnect", "Disconnect", False)], AMBER)
    return h


def _row(o, y, ok, text, right=""):
    x = P + CP
    if ok is True:
        o.append(g_check(x + 5, y - 4, GO, 0.9))
    elif ok is False:
        o.append(g_cross(x + 5, y - 4, RED, 0.9))
    else:
        o.append(g_dot(x + 5, y - 4, DIM))
    o.append(tx(x + 20, y, text, 12, FG))
    if right:
        o.append(tx(P + IW - CP, y, right, 12, DIM, anchor="end"))


def _checks_h(b):
    return CP + 14 + LINE + len(b.probes) * 20 + LINE + 14 + LINE + 2 * 20 + LINE + 14 + LINE + 3 * 20 + LINE + BTN_H + CP


def _checks(o, spots, y, b):
    h = _checks_h(b)
    _card(o, y, h)
    x, cy = P + CP, y + CP + 10
    o.append(_label(x, cy, "WORK · PROBES"))
    o.append(_label(P + IW - CP, cy, "CHECKED JUST NOW", anchor="end"))
    cy += 4 + LINE
    for p in b.probes:
        cy += 20
        _row(o, cy - 6, p.ok, p.target, p.right)
    cy += LINE + 10
    o.append(_label(x, cy, "DNS ON TUN0"))
    cy += 4 + LINE
    for ok, text, right in [(True, "10.8.0.2 10.8.0.3", "resolvers"), (True, "~corp.example.com", "domains")]:
        cy += 20
        _row(o, cy - 6, ok, text, right)
    cy += LINE + 10
    o.append(_label(x, cy, "OPENVPN3'S COUNTERS"))
    cy += 4 + LINE
    for ok, text, right in [(None, "312 MB in, 41 MB out", "transport"), (True, "the server is talking", ""), (None, "0 keepalive timeouts, 1 reconnects", "")]:
        cy += 20
        _row(o, cy - 6, ok, text, right)
    _buttons(o, spots, x, cy + LINE, [("card:check", "Check now", False)])
    return h


def _history_h():
    return CP + 14 + LINE + len(HISTORY) * 20 + CP


def _history(o, y):
    h = _history_h()
    _card(o, y, h)
    x, cy = P + CP, y + CP + 10
    o.append(_label(x, cy, "WHAT HAPPENED"))
    o.append(_label(P + IW - CP, cy, "NEWEST FIRST", anchor="end"))
    cy += 4 + LINE
    for when, what, color, detail in HISTORY:
        cy += 20
        o.append(tx(x, cy - 6, when, 12, DIM))
        o.append(tx(x + 80, cy - 6, what, 12, color, 700))
        dx = x + 80 + text_w(what, 12) + 8
        room = int((P + IW - CP - dx) / 7.2)
        if detail and room > 3:
            d = detail if len(detail) <= room else detail[: room - 1] + "…"
            o.append(tx(dx, cy - 6, d, 12, DIM))
    return h


def drawer_height(b):
    h = P + HEADER_H + GAP
    if b.ask:
        h += _ask_h() + GAP
    h += TABS_H + GAP
    h += {"now": _now_h(b), "checks": _checks_h(b), "history": _history_h()}[b.tab]
    return h + GAP + FOOT_H + P


def draw_drawer(b):
    """One state of the drawer. Returns (svg, height, spots): spots name what
    a story can click, as (x, y, w, h) in drawer coordinates: connect and
    notnow (the request), tab:<now|checks|doctor|history>, and the card's
    buttons as card:<connect|cancel|signin|disconnect|reconnect|check>."""
    o, spots = [], {}
    h = drawer_height(b)
    o.append(f'<rect width="{DRAWER_W}" height="{h}" rx="16" fill="{PANEL}" stroke="{PANEL_LINE}" filter="url(#shadow)"/>')
    color, _, chip = LAMPS[b.lamp]
    y = P
    o.append(f'<circle cx="{P + 4}" cy="{y + 15}" r="4.5" fill="{color}"/>')
    o.append(tx(P + 18, y + 12, "TABLET", 12, FG, 700, spacing=2))
    o.append(tx(P + 18, y + 27, "work", 11, DIM))
    w = text_w(chip, 11) + 18
    o.append(f'<rect x="{P + IW - w:.1f}" y="{y + 5.5}" width="{w:.1f}" height="19" rx="9.5" fill="{color}" fill-opacity=".15"/>')
    o.append(tx(P + IW - w / 2, y + 19, chip, 11, color if b.lamp != "off" else DIM, 700, "middle"))
    y += HEADER_H + GAP
    if b.ask:
        y += _ask(o, spots, y) + GAP
    _tabs(o, spots, y, b.tab)
    y += TABS_H + GAP
    if b.tab == "now":
        y += _now(o, spots, y, b)
    elif b.tab == "checks":
        y += _checks(o, spots, y, b)
    else:
        y += _history(o, y)
    y += GAP
    o.append(tx(P, y + 10, "Enter connect · r reconnect · k check · 1-4 tabs · Esc close", 10.5, DIM))
    return "".join(o), h, spots


def center(spot):
    x, y, w, h = spot
    return x + w / 2, y + h / 2


# ---- end of the panel ----
# dot-files' bar hero (docs/bar/apps.py) runs everything above this line
# to draw the panel in its own tour, so keep the panel's code above it and
# the story below it.

# ---------------------------------------------------------------- animation

# The story, in seconds.
ASK = 0.8  # the agent runs `tablet need work`
CLICK_LAMP, OPEN = 1.5, 1.58  # a right-click on the lamp
CLICK_CONNECT = 3.1
SIGNIN = 4.0
HELD = 5.2
STALE = 7.6
CLICK_RECONNECT = 9.4
HELD2 = 10.4
CLICK_CHECKS = 11.6
CLICK_HISTORY = 13.6
CLICK_OUT, CLOSED = 15.6, 16.05
RESET = 17.2
LAG = 0.1  # a click reaches the daemon and comes back as a snapshot

out, kfs = [], []


def add(s):
    out.append(s)


def pct(sec):
    return f"{max(0.0, min(100.0, sec / T * 100)):.2f}%"


def frames(pts):
    """A keyframe set from (seconds, declarations) points; returns its name."""
    name = f"k{len(kfs)}"
    rows = " ".join(f"{pct(t)} {{ {d}; }}" for t, d in pts)
    kfs.append(f"@keyframes {name} {{ {rows} }}")
    return name


def anim(prop, pts):
    return frames([(t, f"{prop}: {v}") for t, v in pts])


def fade(spans, ramp=0.16, out_ramp=None):
    """Opacity keyframes visible during each (start, end) span."""
    out_ramp = ramp if out_ramp is None else out_ramp
    pts = []
    for s, e in spans:
        if s <= 0:
            pts.append((0, 1))
        else:
            pts += [(0, 0)] if not pts else []
            pts += [(s, 0), (s + ramp, 1)]
        if e >= T:
            pts.append((T, 1))
        else:
            pts += [(e, 1), (e + out_ramp, 0)]
    if pts[-1][0] < T:
        pts.append((T, 0))
    return anim("opacity", pts)


def style(*names, extra=""):
    return f'class="a" style="animation-name:{",".join(names)};{extra}"'


def group(name, shown=True, extra=""):
    """A group that fades by `name`; `shown` is its state in the still."""
    add(f'<g {style(name, extra=extra)} opacity="{1 if shown else 0}">')


def press(t):
    """A button's flash under a click."""
    return anim("opacity", [(0, 0), (t - 0.01, 0), (t, 1), (t + 0.3, 0), (T, 0)])


# ---------------------------------------------------------------- scene

css = f"""
text {{ white-space: pre; }}
.a {{ animation-duration: {T}s; animation-iteration-count: infinite; animation-timing-function: linear; animation-fill-mode: both; }}
.pulse {{ animation: pulse 1.2s ease-in-out infinite; }}
@keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: .4; }} }}
@media (prefers-reduced-motion: reduce) {{
  .a, .pulse {{ animation: none !important; }}
  #cursor, .ripple {{ display: none; }}
}}
"""


def song_defs():
    """Tannoy's song cover, for the media pill."""
    lanterns = ""
    for x, y, s in [(26, 40, 1.0), (54, 28, 1.25), (78, 46, 0.85)]:
        lanterns += (f'<line x1="{x}" y1="0" x2="{x}" y2="{y - 9 * s:.1f}" stroke="#2e2235" stroke-width="1.2"/>'
                     f'<circle cx="{x}" cy="{y}" r="{18 * s:.1f}" fill="url(#glow)"/>'
                     f'<rect x="{x - 6 * s:.1f}" y="{y - 9 * s:.1f}" width="{12 * s:.1f}" height="{18 * s:.1f}" rx="{5 * s:.1f}" fill="#ebcb8b"/>')
    return ('<linearGradient id="dusk" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3b2b4a"/><stop offset=".7" stop-color="#b8706a"/><stop offset="1" stop-color="#d08770"/></linearGradient>'
            '<radialGradient id="glow"><stop offset="0" stop-color="#ebcb8b" stop-opacity=".55"/><stop offset="1" stop-color="#ebcb8b" stop-opacity="0"/></radialGradient>'
            '<clipPath id="art"><rect width="100" height="100" rx="7"/></clipPath>'
            f'<symbol id="song" viewBox="0 0 100 100"><g clip-path="url(#art)"><rect width="100" height="100" fill="url(#dusk)"/>{lanterns}'
            '<path d="M0,74 Q22,64 44,72 T100,68 V100 H0 Z" fill="#2e2235"/></g></symbol>')


add(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="{FONT}" xml:space="preserve" role="img" aria-label="{esc(ALT)}">')
STYLE_AT = len(out)
add("")  # the <style> goes here once every keyframe exists
add(f"""<defs>
<linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{BG0}"/><stop offset="1" stop-color="{BG1}"/></linearGradient>
<radialGradient id="haze1" cx=".85" cy="0" r=".75"><stop offset="0" stop-color="{FROST}" stop-opacity=".13"/><stop offset="1" stop-color="{FROST}" stop-opacity="0"/></radialGradient>
<radialGradient id="haze2" cx=".05" cy="1" r=".6"><stop offset="0" stop-color="{PURPLE}" stop-opacity=".09"/><stop offset="1" stop-color="{PURPLE}" stop-opacity="0"/></radialGradient>
{song_defs()}
<filter id="shadow" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#000" flood-opacity=".65"/></filter>
<clipPath id="screen"><rect width="{W}" height="{H}" rx="18"/></clipPath>
</defs>""")
add('<g clip-path="url(#screen)">')
add(f'<rect width="{W}" height="{H}" fill="url(#wall)"/><rect width="{W}" height="{H}" fill="url(#haze1)"/><rect width="{W}" height="{H}" fill="url(#haze2)"/>')


def window(x, y, w, h, title=""):
    add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" fill="#1a1d22" stroke="#2a2e35"/>')
    add(f'<rect x="{x}" y="{y}" width="{w}" height="30" rx="12" fill="#1f2328"/>')
    for k, c in enumerate([RED, AMBER, GO]):
        add(f'<circle cx="{x + 18 + k * 16}" cy="{y + 15}" r="5" fill="{c}" fill-opacity=".55"/>')
    if title:
        add(tx(x + w / 2, y + 19, title, 11, DIM, anchor="middle"))


# The terminal the agent works in.
TX_, TY_, TW_, TH_ = 34, 36, 560, 330
window(TX_, TY_, TW_, TH_, "claude · ~/Sites/storefront")
TL = TX_ + 18


def term(y, s, c=FG, spans=None, weight=400):
    if spans is None:
        add(tx(TL, y, s, 12.5, c, weight))
    else:
        group(fade(spans, 0.05), spans[0][0] <= 0)
        add(tx(TL, y, s, 12.5, c, weight))
        add("</g>")


ty = TY_ + 58
term(ty, "● I'll build it; Nexus is on the corporate VPN.", DIM)
ty += 26
term(ty, "$ tablet need work --reason \"Nexus only answers on the VPN\" \\", FROST, [(0.3, RESET)])
term(ty + 19, "    -- ./gradlew assemble", FROST, [(0.3, RESET)])
term(ty + 42, "tablet: work is not connected; asking on the bar", AMBER, [(ASK, RESET)])
term(ty + 61, "        (the lamp is amber)", AMBER, [(ASK, RESET)])
term(ty + 84, "tablet: work is connected; running ./gradlew assemble", GO, [(HELD + 0.2, RESET)])
term(ty + 107, "> Task :app:compileKotlin", DIM, [(HELD + 0.7, RESET)])
term(ty + 126, "> Task :app:assemble", DIM, [(HELD + 1.3, RESET)])
term(ty + 149, "BUILD SUCCESSFUL in 41s", GO, [(HELD + 2.0, RESET)], 700)

# The browser finishing the web sign-in openvpn3 opened.
BX_, BY_, BW_, BH_ = 640, 120, 300, 170
group(fade([(SIGNIN + 0.1, HELD + 0.6)], 0.15), False)
window(BX_, BY_, BW_, BH_, "login.example.com")
add(tx(BX_ + BW_ / 2, BY_ + 70, "Sign in to vpn.example.com", 13, FG, 700, "middle"))
group(fade([(SIGNIN + 0.1, HELD - 0.2)], 0.05, 0.05), False)
add(f'<rect x="{BX_ + 90}" y="{BY_ + 96}" width="120" height="30" rx="8" fill="#5e81ac"/>')
add(tx(BX_ + 150, BY_ + 116, "Continue", 12.5, BRIGHT, 700, "middle"))
add("</g>")
group(fade([(HELD - 0.2, HELD + 0.6)], 0.05, 0.05), False)
add(g_check(BX_ + 106, BY_ + 110, GO, 1.3))
add(tx(BX_ + 122, BY_ + 115, "Signed in", 13, GO, 700))
add("</g>")
add("</g>")

# ---------------------------------------------------------------- the bar
# packages/waybar in dot-files: a floating bar 34 px high, 6 px off the
# edges, 6 px between pills. Modules pad 8 px a side inside a group's 4.

BAR_Y, BAR_H = 620, 34
MID = BAR_Y + 21.5  # text baseline
CY = BAR_Y + BAR_H / 2
BAR_SIZE, CW = 13, 7.8  # bar font size and its advance
GW = 11  # a Nerd Font glyph's width in the bar


def pill(x, w, fill=PILL):
    add(f'<rect x="{x:.1f}" y="{BAR_Y + 0.5}" width="{w:.1f}" height="{BAR_H - 1}" rx="10" fill="{fill}" stroke="{PILL_LINE}"/>')


# Left: workspaces and the window title.
x = 6
ws = [("1", True), ("2", False), ("3", False)]
ws_w = 4 + len(ws) * (8 + CW + 8 + 2) + 4
pill(x, ws_w)
bx = x + 4
for num, active in ws:
    w = 8 + CW + 8
    bx += 1
    if active:
        add(f'<rect x="{bx:.1f}" y="{BAR_Y + 4}" width="{w:.1f}" height="{BAR_H - 8}" rx="7" fill="{PILL_ON}"/>')
    add(tx(bx + 8, MID, num, BAR_SIZE, BRIGHT if active else BAR_FG, attrs='fill-opacity=".7"'))
    bx += w + 1
x += ws_w + 6
TITLE = "claude · storefront"
title_w = 12 + len(TITLE) * CW + 12
pill(x, title_w)
add(tx(x + 12, MID, TITLE, BAR_SIZE, BRIGHT))
LEFT_END = x + title_w

# Right, packed from the edge: the tray, the clock, the desk, audio, the
# lamps (whose width follows Tablet's word), then the media pill.
x = W - 6
tray_w = 10 + 3 * 14 + 2 * 8 + 10
x -= tray_w
pill(x, tray_w)
for k, c in enumerate(["#81a1c1", PURPLE, "#5e81ac"]):
    add(f'<rect x="{x + 10 + k * 22}" y="{CY - 7}" width="14" height="14" rx="4" fill="{c}" fill-opacity=".8"/>')
CLOCK = "Fri Nov 13  14:28"
clock_w = 12 + len(CLOCK) * CW + 12
x -= 6 + clock_w
pill(x, clock_w)
add(tx(x + 12, MID, CLOCK, BAR_SIZE, PURPLE))
desk_w = 4 + (16 + GW) + (16 + 8) + 4
x -= 6 + desk_w
pill(x, desk_w)
mx = x + 4 + 8 + GW / 2
add(f'<g fill="none" stroke="{BAR_DIM}" stroke-width="1.5" stroke-linecap="round"><rect x="{mx - 2.8}" y="{CY - 7.5}" width="5.6" height="9.5" rx="2.8"/><path d="M{mx - 5.5},{CY - 1} a5.5,5.5 0 0 0 11,0 M{mx},{CY + 4.5} v3"/></g>')
hx = x + 4 + 16 + GW + 8 + 4
add(f'<path d="M{hx + 2},{CY - 5} l-5,5 l5,5" fill="none" stroke="{BAR_DIM}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>')
vol_w = 16 + GW + CW + 3 * CW
audio_w = 4 + vol_w + 4
x -= 6 + audio_w
pill(x, audio_w)
add(g_speaker(x + 4 + 8 + 7, CY, FROST))
add(tx(x + 4 + 8 + GW + CW, MID, "35%", BAR_SIZE, FROST))
LAMPS_R = x - 6  # the lamps pill ends here

# Tablet's lamp, state by state. Waybar redraws, it does not fade.
LAMP_STATES = [  # (lamp, spans)
    ("off", [(0, ASK), (RESET, T)]),
    ("asking", [(ASK, CLICK_CONNECT + LAG)]),
    ("connecting", [(CLICK_CONNECT + LAG, SIGNIN), (CLICK_RECONNECT + LAG, HELD2)]),
    ("signin", [(SIGNIN, HELD)]),
    ("held", [(HELD, STALE), (HELD2, RESET)]),
    ("stale", [(STALE, CLICK_RECONNECT + LAG)]),
]
LAMP = 16 + GW
SONG_BAR = "Marlow Fen · Paper Lanterns"
TABLET_X = {}
for lamp, spans in LAMP_STATES:
    color, word, _ = LAMPS[lamp]
    extra = (CW + len(word) * CW) if word else 0
    lamps_w = 4 + 5 * LAMP + extra + 4
    lx = LAMPS_R - lamps_w
    still = any(s <= HELD + 1 <= e for s, e in spans)  # the reduced-motion still
    group(fade(spans, 0.01, 0.01), still)
    pill(lx, lamps_w)
    cx = [lx + 4 + LAMP / 2 + i * LAMP for i in range(4)]
    cx[3] = lx + 4 + 3 * LAMP + 8 + GW / 2
    bell = lx + 4 + 4 * LAMP + extra + LAMP / 2
    add(f'<circle cx="{cx[0]}" cy="{CY}" r="5.5" fill="{GO}"/>')
    add(f'<rect x="{cx[1] - 5}" y="{CY - 9}" width="10" height="18" rx="4" fill="none" stroke="{FROST}" stroke-width="1.4"/>')
    for dy in (-5, 0, 5):
        add(f'<circle cx="{cx[1]}" cy="{CY + dy}" r="1.8" fill="{FROST}"/>')
    add(f'<circle cx="{cx[2]}" cy="{CY}" r="5.5" fill="none" stroke="{BAR_DIM}" stroke-width="1.6"/>')
    pulse = ' class="pulse"' if lamp in ("asking", "connecting", "signin") else ""
    add(f"<g{pulse}>")
    add(g_lan(cx[3], CY, color, connected=lamp in ("held",)))
    if word:
        add(tx(cx[3] + GW / 2 + CW, MID, word, BAR_SIZE, color))
    add("</g>")
    add(g_bell(bell, CY, BAR_DIM))
    TABLET_X[lamp] = cx[3]
    # The media pill sits left of the lamps.
    w = 10 + 18 + 8 + len(SONG_BAR) * CW + 12
    px = lx - 6 - w
    pill(px, w)
    add(f'<use href="#song" x="{px + 10:.1f}" y="{CY - 9}" width="18" height="18"/>')
    add(tx(px + 36, MID, SONG_BAR, BAR_SIZE, AMBER))
    assert px - 6 >= LEFT_END, f"the bar overflows by {LEFT_END - px + 6:.0f}px in {lamp}"
    add("</g>")

# ---------------------------------------------------------------- the drawer
STATES = [  # (board, spans)
    (Board("asking", phase="off", ask=True, big="off"), [(0, CLICK_CONNECT + LAG)]),
    (Board("connecting", phase="connecting", big="1s…", reason="starting openvpn3"), [(CLICK_CONNECT + LAG, SIGNIN)]),
    (Board("signin", phase="signin", big="waiting on you", reason="waiting for the web sign-in"), [(SIGNIN, HELD)]),
    (Board("held", big="0s"), [(HELD, STALE)]),
    (Board("stale", phase="stale", big="3m", probes=PROBES_BAD,
           reason="openvpn3 says connected, but all 2 probes failed twice in a row",
           graph=(26, 30, 21, 17, 24, 6, 2, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0)), [(STALE, CLICK_RECONNECT + LAG)]),
    (Board("connecting", phase="connecting", big="1s…", reason="starting openvpn3"), [(CLICK_RECONNECT + LAG, HELD2)]),
    (Board("held", big="1s", graph=(0, 0, 0, 1, 0, 3, 9, 14, 11, 22, 18, 12, 16, 26, 30, 21, 17, 24, 28, 31)), [(HELD2, CLICK_CHECKS + LAG)]),
    (Board("held", tab="checks"), [(CLICK_CHECKS + LAG, CLICK_HISTORY + LAG)]),
    (Board("held", tab="history"), [(CLICK_HISTORY + LAG, T)]),
]
DH = max(drawer_height(b) for b, _ in STATES)
LAMP_X = TABLET_X["off"]
DX = min(max(10, LAMP_X - DRAWER_W / 2), W - 10 - DRAWER_W)
DY = BAR_Y - 8 - DH

spring = anim("transform", [
    (0, "translateY(26px) scale(.06)"), (OPEN - 0.01, "translateY(26px) scale(.06)"),
    (OPEN + 0.10, "translateY(14px) scale(.5)"), (OPEN + 0.20, "translateY(4px) scale(.9)"),
    (OPEN + 0.30, "translateY(-1px) scale(1.035)"), (OPEN + 0.42, "translateY(0) scale(.988)"),
    (OPEN + 0.56, "scale(1.006)"), (OPEN + 0.72, "scale(1)"), (CLICK_OUT + 0.08, "scale(1)"),
    (CLOSED, "translateY(14px) scale(.4)"), (T, "translateY(14px) scale(.4)"),
])
show = anim("opacity", [(0, 0), (OPEN - 0.01, 0), (OPEN + 0.12, 0.85), (OPEN + 0.2, 1), (CLICK_OUT + 0.08, 1), (CLOSED, 0), (T, 0)])
# The still (reduced motion) shows the drawer holding the tunnel.
add(f'<g {style(spring, show, extra=f"transform-origin:{LAMP_X:.1f}px {BAR_Y}px")}><g transform="translate({DX:.1f},{DY:.1f})">')
SPOTS = []
for i, (b, spans) in enumerate(STATES):
    svg, h, spots = draw_drawer(b)
    SPOTS.append((spots, DH - h))
    end = spans[0][1] + 0.04 if i + 1 < len(STATES) else T
    group(fade([(spans[0][0], end)], 0.04, 0.01), b.lamp == "held" and b.tab == "now" and b.big == "0s")
    add(f'<g transform="translate(0,{DH - h})">{svg}</g>')
    add("</g>")


def spot(i, name):
    spots, dy = SPOTS[i]
    cx, cy = center(spots[name])
    return cx, cy + dy


def flash(i, name, t):
    spots, dy = SPOTS[i]
    x, y, w, h = spots[name]
    add(f'<rect x="{x:.1f}" y="{y + dy:.1f}" width="{w}" height="{h}" rx="8" fill="#fff" fill-opacity=".16" {style(press(t))} opacity="0"/>')


flash(0, "connect", CLICK_CONNECT)
flash(4, "card:reconnect", CLICK_RECONNECT)
flash(6, "tab:checks", CLICK_CHECKS)
flash(7, "tab:history", CLICK_HISTORY)
add("</g></g>")

# ---------------------------------------------------------------- cursor


def screen(p):
    return (DX + p[0], DY + p[1])


lamp_c = (LAMP_X, CY)
connect_c = screen(spot(0, "connect"))
reconnect_c = screen(spot(4, "card:reconnect"))
checks_c = screen(spot(6, "tab:checks"))
history_c = screen(spot(7, "tab:history"))
rest = (DX - 60, DY + 120)
outside = (DX - 140, 430)
clicks = [(CLICK_LAMP, *lamp_c), (CLICK_CONNECT, *connect_c), (CLICK_RECONNECT, *reconnect_c), (CLICK_CHECKS, *checks_c),
          (CLICK_HISTORY, *history_c), (CLICK_OUT, *outside)]
for t, cx_, cy_ in clicks:
    k = anim("opacity", [(0, 0), (t - 0.01, 0), (t, 0.9), (t + 0.45, 0), (T, 0)])
    k2 = anim("transform", [(0, "scale(.3)"), (t, "scale(.3)"), (t + 0.45, "scale(1.6)"), (T, "scale(1.6)")])
    add(f'<circle class="a ripple" style="animation-name:{k},{k2};transform-box:fill-box;transform-origin:center" cx="{cx_:.1f}" cy="{cy_:.1f}" r="18" fill="none" stroke="{FROST}" stroke-width="2.5" opacity="0"/>')


def tr(p):
    return f"translate({p[0] - 3:.1f}px, {p[1] - 4:.1f}px)"


home = (DX - 100, 480)
cur = anim("transform", [
    (0, tr(home)), (CLICK_LAMP - 0.2, tr(lamp_c)), (2.3, tr(lamp_c)),
    (CLICK_CONNECT - 0.2, tr(connect_c)), (3.6, tr(connect_c)),
    (4.4, tr(rest)), (8.7, tr(rest)),
    (CLICK_RECONNECT - 0.2, tr(reconnect_c)), (10.0, tr(reconnect_c)),
    (CLICK_CHECKS - 0.2, tr(checks_c)), (12.4, tr(checks_c)),
    (CLICK_HISTORY - 0.2, tr(history_c)), (14.6, tr(history_c)),
    (CLICK_OUT - 0.2, tr(outside)), (16.6, tr(outside)), (T, tr(home)),
])
add(f'<g id="cursor" class="a" style="animation-name:{cur};animation-timing-function:cubic-bezier(.45,0,.25,1);transform:{tr(home)}">')
add('<path d="M0,0 L0,22 L6,17 L10,26 L14,24 L10,15 L18,15 Z" fill="#eceff4" stroke="#1d2024" stroke-width="1.4" stroke-linejoin="round"/>')
add("</g>")
add("</g></svg>")

out[STYLE_AT] = "<style>" + css + "\n".join(kfs) + "</style>"
Path(__file__).with_name("hero.svg").write_text("\n".join(out) + "\n")
print(f"wrote hero.svg ({sum(len(s) + 1 for s in out) // 1024} KB, {len(kfs)} keyframe sets); "
      f"drawer {DRAWER_W}x{DH} at ({DX:.0f}, {DY:.0f})")
