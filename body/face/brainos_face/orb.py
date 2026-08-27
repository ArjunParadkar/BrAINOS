"""The breathing orb: state colour, tier ring, waking pulse, press-and-hold → PTT."""
import math, time
import cairo
from gi.repository import Gtk, Gdk, GLib
from . import theme

TIER_SEG = {"reflex": 1, "fast": 2, "deliberate": 3}

class Orb(Gtk.DrawingArea):
    def __init__(self, on_ptt, size=36):
        super().__init__()
        self.size = size; self.set_content_width(size); self.set_content_height(size)
        self.mode = "waking"; self.tier = None; self.t0 = time.monotonic()
        self._col = theme.rgb(theme.DIM); self._target = self._col
        self.on_ptt = on_ptt; self._press_t = None; self._ptt = False
        self.set_draw_func(self._draw)
        self.set_cursor(Gdk.Cursor.new_from_name("pointer"))
        g = Gtk.GestureClick(); g.set_button(1)
        g.connect("pressed", self._pressed); g.connect("released", self._released); g.connect("cancel", lambda *_: self._released(None, 0, 0, 0))
        self.add_controller(g)
        GLib.timeout_add(33, self._tick)

    def set_state(self, mode, tier=None):
        self.mode = mode; self.tier = tier
        self._target = theme.rgb(theme.MODE_COLOR.get(mode, theme.ACCENT))

    # press-and-hold on the orb = push-to-talk
    def _pressed(self, g, n, x, y):
        self._press_t = time.monotonic()
        GLib.timeout_add(250, self._hold_check)
    def _hold_check(self):
        if self._press_t is not None and not self._ptt:
            self._ptt = True; self.on_ptt(True)
        return False
    def _released(self, g, n, x, y):
        self._press_t = None
        if self._ptt:
            self._ptt = False; self.on_ptt(False)

    def _tick(self):
        c, t = self._col, self._target
        self._col = tuple(a + (b - a) * 0.15 for a, b in zip(c, t))
        self.queue_draw(); return True

    def _draw(self, area, cr, W, H):
        t = time.monotonic() - self.t0
        cx, cy = W / 2, H / 2
        r, g, b = self._col
        if self.mode == "waking": breath = 0.5 + 0.5 * math.sin(t * 1.6)
        elif self.mode == "listening": breath = 0.55 + 0.45 * math.sin(t * 5.0)
        elif self.mode == "thinking": breath = 0.5 + 0.5 * math.sin(t * 3.2)
        elif self.mode == "acting": breath = 0.6 + 0.4 * math.sin(t * 7.0)
        else: breath = 0.5 + 0.5 * math.sin(t * 1.1)
        R = W * 0.26 + W * 0.05 * breath
        # glow halo
        gl = cairo.RadialGradient(cx, cy, R * 0.6, cx, cy, W * 0.5)
        gl.add_color_stop_rgba(0, r, g, b, 0.55 * (0.5 + 0.5 * breath)); gl.add_color_stop_rgba(1, r, g, b, 0)
        cr.set_source(gl); cr.arc(cx, cy, W * 0.5, 0, 2 * math.pi); cr.fill()
        # core sphere
        core = cairo.RadialGradient(cx - R * 0.35, cy - R * 0.35, R * 0.1, cx, cy, R)
        core.add_color_stop_rgba(0, min(1, r + 0.45), min(1, g + 0.45), min(1, b + 0.45), 1)
        core.add_color_stop_rgba(0.7, r, g, b, 1); core.add_color_stop_rgba(1, r * 0.55, g * 0.55, b * 0.55, 1)
        cr.set_source(core); cr.arc(cx, cy, R, 0, 2 * math.pi); cr.fill()
        # tier ring: 1..3 arc segments, subtle
        n = TIER_SEG.get(self.tier or "", 0)
        if n and self.mode != "waking":
            cr.set_line_width(1.6); cr.set_source_rgba(r, g, b, 0.75)
            rr = W * 0.46; gap = 0.35; span = (2 * math.pi - n * gap) / n
            rot = t * (0.6 if self.mode in ("thinking", "acting") else 0.15)
            for i in range(n):
                a0 = rot + i * (span + gap)
                cr.arc(cx, cy, rr, a0, a0 + span); cr.stroke()
        # thinking: orbiting spark
        if self.mode == "thinking":
            a = t * 4.0; cr.set_source_rgba(1, 1, 1, 0.9)
            cr.arc(cx + math.cos(a) * R * 1.35, cy + math.sin(a) * R * 1.35, 1.8, 0, 2 * math.pi); cr.fill()
