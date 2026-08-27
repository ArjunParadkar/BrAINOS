"""Teach overlay: a full-screen, click-through transparent layer that glows a rounded
highlight around a rect (output-logical px) with a caption pill beside it."""
import math, time
import cairo
from gi.repository import Gtk, Gdk, GLib, Pango, PangoCairo
from . import theme
try:
    from gi.repository import Gtk4LayerShell as LS
except Exception:
    LS = None

class Teach:
    def __init__(self):
        self.win = None; self.rect = None; self.caption = ""; self.t0 = 0; self.alpha = 0; self.want = 0; self._src = None

    def _ensure(self):
        if self.win: return
        w = self.win = Gtk.Window(); w.set_decorated(False)
        if LS:
            LS.init_for_window(w); LS.set_namespace(w, "brainos-teach"); LS.set_layer(w, LS.Layer.OVERLAY)
            for e in (LS.Edge.TOP, LS.Edge.BOTTOM, LS.Edge.LEFT, LS.Edge.RIGHT): LS.set_anchor(w, e, True)
            LS.set_exclusive_zone(w, -1); LS.set_keyboard_mode(w, LS.KeyboardMode.NONE)
        self.area = Gtk.DrawingArea(); self.area.set_draw_func(self._draw); w.set_child(self.area)
        w.connect("realize", lambda *_: GLib.idle_add(self._clear_input))
        w.present(); GLib.idle_add(self._clear_input)

    def _clear_input(self):
        s = self.win.get_surface() if self.win else None
        if s: s.set_input_region(cairo.Region())
        return False

    def show(self, m):
        self._ensure()
        self.rect = (float(m.get("x", 0)), float(m.get("y", 0)), float(m.get("w", 0)), float(m.get("h", 0)))
        self.caption = m.get("caption", "") or ""; self.t0 = time.monotonic(); self.want = 1.0
        if not self._src: self._src = GLib.timeout_add(33, self._tick)

    def clear(self):
        self.want = 0.0
        if self.win and not self._src: self._src = GLib.timeout_add(33, self._tick)

    def _tick(self):
        self.alpha += (self.want - self.alpha) * 0.2
        if abs(self.want - self.alpha) < 0.01: self.alpha = self.want
        if self.area: self.area.queue_draw()
        if self.alpha == self.want and self.want == 0.0:
            self._src = None
            if self.win: self.win.destroy(); self.win = None; self.area = None
            return False
        if self.alpha == self.want and self.want == 1.0 and False: pass
        return True

    def _rr(self, cr, x, y, w, h, r):
        cr.new_sub_path(); cr.arc(x + w - r, y + r, r, -math.pi / 2, 0); cr.arc(x + w - r, y + h - r, r, 0, math.pi / 2)
        cr.arc(x + r, y + h - r, r, math.pi / 2, math.pi); cr.arc(x + r, y + r, r, math.pi, 3 * math.pi / 2); cr.close_path()

    def _draw(self, area, cr, W, H):
        if not self.rect or self.alpha <= 0.01: return
        a = self.alpha; t = time.monotonic() - self.t0
        x, y, w, h = self.rect; pr, pg, pb = theme.rgb(theme.ACCENT)
        # dim everything except the rect (soft spotlight)
        cr.set_source_rgba(0, 0, 0, 0.42 * a); cr.set_fill_rule(cairo.FILL_RULE_EVEN_ODD)
        cr.rectangle(0, 0, W, H); self._rr(cr, x - 6, y - 6, w + 12, h + 12, 14); cr.fill()
        cr.set_fill_rule(cairo.FILL_RULE_WINDING)
        # glow rings
        pulse = 0.5 + 0.5 * math.sin(t * 2.4)
        for i, (sp, al, lw) in enumerate(((0, 0.95, 2.5), (6 + 4 * pulse, 0.45, 6), (14 + 8 * pulse, 0.18, 12))):
            cr.set_source_rgba(pr, pg, pb, al * a); cr.set_line_width(lw)
            self._rr(cr, x - 6 - sp, y - 6 - sp, w + 12 + 2 * sp, h + 12 + 2 * sp, 14 + sp); cr.stroke()
        if not self.caption: return
        # caption pill: right of rect if room, else below
        layout = PangoCairo.create_layout(cr)
        fd = Pango.FontDescription(); fd.set_family(theme.FONT.split(",")[0].strip('"')); fd.set_absolute_size(18 * Pango.SCALE); fd.set_weight(Pango.Weight.MEDIUM)
        layout.set_font_description(fd); layout.set_text(self.caption, -1); layout.set_width(int(min(420, W * 0.3) * Pango.SCALE)); layout.set_wrap(Pango.WrapMode.WORD_CHAR)
        tw, th = layout.get_pixel_size(); pw, ph = tw + 36, th + 20
        if x + w + 24 + pw < W: px, py = x + w + 24, y
        elif x - 24 - pw > 0: px, py = x - 24 - pw, y
        else: px, py = x, (y + h + 24 if y + h + 24 + ph < H else y - 24 - ph)
        py = max(8, min(py, H - ph - 8))
        bg = theme.rgb(theme.BG)
        cr.set_source_rgba(0, 0, 0, 0.35 * a); self._rr(cr, px + 2, py + 4, pw, ph, 16); cr.fill()
        cr.set_source_rgba(*bg, 0.96 * a); self._rr(cr, px, py, pw, ph, 16); cr.fill()
        cr.set_source_rgba(pr, pg, pb, 0.8 * a); cr.set_line_width(1.5); self._rr(cr, px, py, pw, ph, 16); cr.stroke()
        # connector dot
        cr.set_source_rgba(pr, pg, pb, a); cr.arc(px if px > x + w else px + pw, py + 18, 4, 0, 2 * math.pi); cr.fill()
        cr.set_source_rgba(*theme.rgb(theme.FG), a); cr.move_to(px + 18, py + 10); PangoCairo.show_layout(cr, layout)
