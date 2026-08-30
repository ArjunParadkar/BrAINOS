"""The presence bar: orb + command entry + streaming reply + action ledger + task panel +
need_key modal. Bottom layer-shell overlay that grows upward while active."""
import time
from gi.repository import Gtk, Gdk, GLib, Pango
from . import theme
from .markup import md_to_pango
from .orb import Orb

try:
    from gi.repository import Gtk4LayerShell as LS
except Exception:  # pragma: no cover
    LS = None

BAR_W = 840; COLLAPSED_H = 44; MARGIN = 14; IDLE_COLLAPSE_MS = 7000
GLYPH = {"pending": "○", "running": "◐", "done": "✓", "failed": "✕"}
ACT_GLYPH = {"ok": "✓", "grant": "›", "deny": "⊘", "err": "!"}

class Bar:
    def __init__(self, send):
        self.send = send
        self.mode = "waking"; self.tier = None; self.connected = False
        self.reply_id = None; self.reply_text = ""; self._collapse_src = None; self._keymode_ex = False
        self._build()

    # ── construction ────────────────────────────────────────────────────────
    def _build(self):
        w = self.win = Gtk.Window(); w.set_decorated(False); w.set_resizable(False)
        w.set_default_size(BAR_W, -1)
        if LS:
            LS.init_for_window(w); LS.set_namespace(w, "brainos-face")
            LS.set_layer(w, LS.Layer.TOP); LS.set_anchor(w, LS.Edge.BOTTOM, True)
            LS.set_margin(w, LS.Edge.BOTTOM, MARGIN)
            LS.set_exclusive_zone(w, COLLAPSED_H + MARGIN)
            LS.set_keyboard_mode(w, LS.KeyboardMode.ON_DEMAND)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL); root.add_css_class("bar"); root.set_size_request(BAR_W, -1)
        w.set_child(root)

        # expanded area (revealer)
        self.rev = Gtk.Revealer(); self.rev.set_transition_type(Gtk.RevealerTransitionType.SLIDE_UP)
        self.rev.set_transition_duration(180); self.rev.set_reveal_child(False)
        exp = Gtk.Box(orientation=Gtk.Orientation.VERTICAL); self.rev.set_child(exp)

        # need_key modal
        self.keybox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6); self.keybox.add_css_class("keybox"); self.keybox.set_visible(False)
        h = Gtk.Label(label="I have no Anthropic API key yet.", xalign=0); h.add_css_class("h")
        p = Gtk.Label(label="Paste it here — it is stored only on the secure partition, never logged. Without it I still run reflex + fast tiers.", xalign=0, wrap=True); p.add_css_class("p")
        row = Gtk.Box(spacing=8)
        self.keyentry = Gtk.PasswordEntry(); self.keyentry.set_show_peek_icon(True); self.keyentry.set_hexpand(True)
        self.keyentry.set_property("placeholder-text", "sk-ant-…"); self.keyentry.connect("activate", self._submit_key)
        b = Gtk.Button(label="Keep it"); b.add_css_class("pink"); b.connect("clicked", self._submit_key)
        later = Gtk.Button(label="later"); later.add_css_class("ghost"); later.connect("clicked", lambda *_: self._hide_key())
        row.append(self.keyentry); row.append(b); row.append(later)
        for x in (h, p, row): self.keybox.append(x)
        exp.append(self.keybox)

        # streaming reply
        # what the ears heard (the human's own line, shown above the reply)
        self.heard = Gtk.Label(xalign=0, wrap=True, wrap_mode=Pango.WrapMode.WORD_CHAR, selectable=True)
        self.heard.add_css_class("heard"); self.heard.set_max_width_chars(60); self.heard.set_visible(False)
        exp.append(self.heard)
        self.reply = Gtk.Label(xalign=0, wrap=True, wrap_mode=Pango.WrapMode.WORD_CHAR, selectable=True, use_markup=True)
        self.reply.add_css_class("reply"); self.reply.set_max_width_chars(60)
        self.reply_scroll = Gtk.ScrolledWindow(); self.reply_scroll.add_css_class("reply-scroll")
        self.reply_scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        self.reply_scroll.set_propagate_natural_height(True); self.reply_scroll.set_max_content_height(340)
        self.reply_scroll.set_child(self.reply); self.reply_scroll.set_visible(False)
        exp.append(self.reply_scroll)

        # task panel
        self.task = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2); self.task.add_css_class("task"); self.task.set_visible(False)
        self.task_title = Gtk.Label(xalign=0); self.task_title.add_css_class("task-title"); self.task.append(self.task_title)
        self.task_steps = Gtk.Box(orientation=Gtk.Orientation.VERTICAL); self.task.append(self.task_steps)
        exp.append(self.task)

        # action ledger
        self.ledger = Gtk.Box(spacing=6); self.ledger.add_css_class("ledger"); self.ledger.set_visible(False)
        exp.append(self.ledger)
        self.sep = Gtk.Separator(); self.sep.add_css_class("sep"); exp.append(self.sep)
        root.append(self.rev)

        # bottom row
        row = Gtk.Box(spacing=8); row.add_css_class("row"); row.set_size_request(-1, COLLAPSED_H)
        self.orb = Orb(self._ptt); self.orb.set_valign(Gtk.Align.CENTER); row.append(self.orb)
        self.entry = Gtk.Entry(); self.entry.add_css_class("cmd"); self.entry.set_hexpand(True)
        self.entry.set_property("placeholder-text", "ask, or tell me what to do")
        self.entry.connect("activate", self._submit)
        kc = Gtk.EventControllerKey(); kc.connect("key-pressed", self._key); self.entry.add_controller(kc)
        fc = Gtk.EventControllerFocus(); fc.connect("leave", lambda *_: self._entry_blur()); self.entry.add_controller(fc)
        row.append(self.entry)
        self.tier_lbl = Gtk.Label(); self.tier_lbl.add_css_class("tier"); row.append(self.tier_lbl)
        self.status = Gtk.Label(label="WAKING…"); self.status.add_css_class("status"); row.append(self.status)
        root.append(row)
        w.present()

    # ── entry / keys ────────────────────────────────────────────────────────
    def focus(self):
        if LS and not self._keymode_ex:
            LS.set_keyboard_mode(self.win, LS.KeyboardMode.EXCLUSIVE); self._keymode_ex = True
        self.win.present(); self.entry.grab_focus(); self._cancel_collapse()
    def _entry_blur(self):
        if LS and self._keymode_ex:
            LS.set_keyboard_mode(self.win, LS.KeyboardMode.ON_DEMAND); self._keymode_ex = False
        self._schedule_collapse()
    def _key(self, c, keyval, code, state):
        if keyval == Gdk.KEY_Escape:
            if self.mode in ("thinking", "acting"): self.send({"t": "cancel"})
            self.entry.set_text(""); self._entry_blur(); self.collapse(); return True
        return False
    def _submit(self, *_):
        text = self.entry.get_text().strip()
        if not text: return
        self.send({"t": "utter", "text": text}); self.entry.set_text("")
        self.heard.set_visible(False); self.reply_id = None; self._set_reply(""); self.expand()
    def _ptt(self, on):
        self.send({"t": "ptt", "on": bool(on)})
        if on: self.expand()
    def _submit_key(self, *_):
        k = self.keyentry.get_text().strip()
        if not k: return
        self.send({"t": "set_key", "key": k}); self.keyentry.set_text(""); self._hide_key()
    def _hide_key(self):
        self.keybox.set_visible(False); self._maybe_collapse()

    # ── expand / collapse ───────────────────────────────────────────────────
    def expand(self):
        self.rev.set_reveal_child(True); self._cancel_collapse()
    def collapse(self):
        self.rev.set_reveal_child(False)
    def _cancel_collapse(self):
        if self._collapse_src: GLib.source_remove(self._collapse_src); self._collapse_src = None
    def _schedule_collapse(self):
        self._cancel_collapse()
        self._collapse_src = GLib.timeout_add(IDLE_COLLAPSE_MS, self._collapse_tick)
    def _collapse_tick(self):
        self._collapse_src = None; self._maybe_collapse(force=True); return False
    def _maybe_collapse(self, force=False):
        busy = self.mode in ("listening", "thinking", "acting") or self.keybox.get_visible() or self.entry.has_focus() \
            or (self.task.get_visible() and not getattr(self, "_task_done", True))
        if busy: return
        if force: self.collapse()
        else: self._schedule_collapse()
    def _anything(self):
        return any(x.get_visible() for x in (self.keybox, self.heard, self.reply_scroll, self.task, self.ledger))

    # ── operator messages ───────────────────────────────────────────────────
    def on_conn(self, up):
        self.connected = up
        if up:
            self.set_state("idle", None); self.status.set_label("READY")
        else:
            self.set_state("waking", None); self.status.set_label("WAKING…")
        self.status.remove_css_class("live")

    def set_state(self, mode, tier):
        self.mode = mode; self.tier = tier
        self.orb.set_state(mode, tier)
        self.tier_lbl.set_label((tier or "").upper())
        label = {"idle": "READY", "listening": "LISTENING", "thinking": "THINKING", "acting": "ACTING", "waking": "WAKING…"}.get(mode, mode.upper())
        self.status.set_label(label)
        if mode in ("listening", "thinking", "acting"): self.status.add_css_class("live"); self.expand()
        else:
            self.status.remove_css_class("live")
            if self._anything(): self._schedule_collapse()

    def _set_reply(self, text):
        self.reply_text = text
        if text:
            self.reply.set_markup(md_to_pango(text, mono=theme.MONO.split(",")[0].strip('"'), accent=theme.ACCENT, dim=theme.DIM))
            self.reply_scroll.set_visible(True)
            GLib.idle_add(self._scroll_end)
        else:
            self.reply.set_markup(""); self.reply_scroll.set_visible(False)
    def _scroll_end(self):
        adj = self.reply_scroll.get_vadjustment(); adj.set_value(adj.get_upper() - adj.get_page_size()); return False

    def on_heard(self, m):
        text = (m.get("text") or "").strip()
        self.heard.set_label("\u201c" + text + "\u201d" if text else "")
        self.heard.set_visible(bool(text))
        self._set_reply("")          # a new voice turn starts a fresh reply
        self.expand()

    def on_say(self, m):
        sid = m.get("id"); text = m.get("text", "") or ""
        if m.get("delta") and sid == self.reply_id: text = self.reply_text + text
        elif m.get("delta") and self.reply_id is None: text = self.reply_text + text
        self.reply_id = sid
        self._set_reply(text); self.expand()
        if m.get("done"): self._schedule_collapse()

    def on_act(self, m):
        status = m.get("status", "ok"); verb = m.get("verb", "?"); target = m.get("target", "") or ""
        chip = Gtk.Box(spacing=5); chip.add_css_class("chip"); chip.add_css_class(status)
        v = Gtk.Label(label=verb); v.add_css_class("verb"); chip.append(v)
        if target: chip.append(Gtk.Label(label=target[:40], ellipsize=Pango.EllipsizeMode.END))
        chip.append(Gtk.Label(label=ACT_GLYPH.get(status, "·")))
        if status == "deny": chip.set_tooltip_text("KIRA denied: " + (m.get("digest") or ""))
        elif m.get("digest"): chip.set_tooltip_text(m["digest"])
        self.ledger.append(chip)
        while len(list(self.ledger)) > 5: self.ledger.remove(self.ledger.get_first_child())
        self.ledger.set_visible(True); self.expand(); self._schedule_collapse()

    def on_task(self, m):
        self._task_done = bool(m.get("done"))
        self.task_title.set_label(("DONE · " if self._task_done else "PLAN · ") + (m.get("title") or ""))
        if self._task_done: self.task_title.add_css_class("done")
        else: self.task_title.remove_css_class("done")
        while (c := self.task_steps.get_first_child()): self.task_steps.remove(c)
        for st in m.get("steps", []):
            s = st.get("status", "pending"); row = Gtk.Box(spacing=6); row.add_css_class("step"); row.add_css_class(s)
            g = Gtk.Label(label=GLYPH.get(s, "○")); g.add_css_class("glyph"); row.append(g)
            row.append(Gtk.Label(label=st.get("title", ""), xalign=0, ellipsize=Pango.EllipsizeMode.END))
            self.task_steps.append(row)
        self.task.set_visible(True); self.expand()
        if self._task_done: self._schedule_collapse()

    def on_body(self, m):
        """Stage 4: another device became (or stopped being) a body of this entity."""
        name = m.get("name", "?"); joined = bool(m.get("joined"))
        chip = Gtk.Box(spacing=5); chip.add_css_class("chip"); chip.add_css_class("ok" if joined else "err")
        v = Gtk.Label(label="body"); v.add_css_class("verb"); chip.append(v)
        chip.append(Gtk.Label(label=f"{name} {'joined' if joined else 'left'}"))
        chip.append(Gtk.Label(label="+" if joined else "-"))
        if joined: chip.set_tooltip_text(f"{m.get('os','?')} · {len(m.get('hands', []))} hands · {m.get('rtt_ms', 0)} ms")
        self.ledger.append(chip)
        while len(list(self.ledger)) > 5: self.ledger.remove(self.ledger.get_first_child())
        self.ledger.set_visible(True); self.expand(); self._schedule_collapse()

    def on_pool(self, m):
        """Stage 4: which brains are up — surfaced as the tier label's tooltip."""
        models = m.get("models") or []
        txt = " · ".join(f"{x.get('role')}: {x.get('model') or x.get('kind')} ({'ok' if x.get('ok') else 'down'})" for x in models)
        self.tier_lbl.set_tooltip_text(txt or None)

    def on_need_key(self):
        self.keybox.set_visible(True); self.expand(); self.focus_key()
    def focus_key(self):
        if LS and not self._keymode_ex:
            LS.set_keyboard_mode(self.win, LS.KeyboardMode.EXCLUSIVE); self._keymode_ex = True
        self.keyentry.grab_focus()
