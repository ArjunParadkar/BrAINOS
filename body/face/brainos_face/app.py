"""GApplication shell: single instance; `--focus`, `--key-down`, `--key-up` are remote
actions delivered to the running face (D-Bus when present; otherwise a tiny unix socket
at $XDG_RUNTIME_DIR/brainos/face.sock)."""
import ctypes, os, socket, sys, time, threading
import gi
try:  # gtk4-layer-shell must be loaded before Gtk touches wayland
    ctypes.CDLL("libgtk4-layer-shell.so", mode=ctypes.RTLD_GLOBAL)
    gi.require_version("Gtk4LayerShell", "1.0")
except OSError:
    pass
gi.require_version("Gtk", "4.0"); gi.require_version("Gdk", "4.0")
from gi.repository import Gtk, Gdk, Gio, GLib
from . import theme
from .ipc import Link
from .bar import Bar
from .surface import Surfaces
from .teach import Teach

APP_ID = "com.arenda.brainos.face"
HOLD_MS = 250

def ctl_path():
    rt = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    return os.path.join(rt, "brainos", "face.sock")

class Face:
    def __init__(self, app):
        self.app = app
        self.link = Link(self.on_msg, self.on_conn)
        self.bar = Bar(self.link.send); self.surfaces = Surfaces(self.link.send); self.teach = Teach()
        self.bar.win.set_application(app)
        self._key_t = None; self._ptt = False
        css = Gtk.CssProvider(); css.load_from_string(theme.css())
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        threading.Thread(target=self._ctl_server, daemon=True).start()

    # ── remote control (hotkey) ─────────────────────────────────────────────
    def cmd(self, name):
        if name == "focus": self.bar.focus()
        elif name == "key-down":
            self._key_t = time.monotonic(); GLib.timeout_add(HOLD_MS, self._hold)
        elif name == "key-up":
            if self._ptt: self._ptt = False; self.link.send({"t": "ptt", "on": False})
            elif self._key_t is not None: self.bar.focus()
            self._key_t = None
        elif name == "ptt-on": self._ptt = True; self.link.send({"t": "ptt", "on": True}); self.bar.expand()
        elif name == "ptt-off": self._ptt = False; self.link.send({"t": "ptt", "on": False})
        elif name == "cancel": self.link.send({"t": "cancel"})
        return False
    def _hold(self):
        if self._key_t is not None and not self._ptt:
            self._ptt = True; self.link.send({"t": "ptt", "on": True}); self.bar.expand()
        return False
    def _ctl_server(self):
        p = ctl_path(); os.makedirs(os.path.dirname(p), exist_ok=True)
        try: os.unlink(p)
        except OSError: pass
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.bind(p); s.listen(4)
        while True:
            c, _ = s.accept()
            try:
                name = c.recv(64).decode().strip()
                if name: GLib.idle_add(self.cmd, name)
            finally: c.close()

    # ── operator → face ─────────────────────────────────────────────────────
    def on_conn(self, up):
        self.bar.on_conn(up); return False
    def on_msg(self, m):
        t = m.get("t")
        if t == "state": self.bar.set_state(m.get("mode", "idle"), m.get("tier"))
        elif t == "say": self.bar.on_say(m)
        elif t == "act": self.bar.on_act(m)
        elif t == "surface": self.surfaces.handle(m)
        elif t == "teach":
            if m.get("clear"): self.teach.clear()
            else: self.teach.show(m)
        elif t == "task": self.bar.on_task(m)
        elif t == "need_key": self.bar.on_need_key()
        return False

def _remote(name):
    """Deliver a command to the running face. Returns True if delivered."""
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(0.5); s.connect(ctl_path()); s.sendall(name.encode()); s.close(); return True
    except OSError:
        return False

def main(argv):
    cmds = [a[2:] for a in argv[1:] if a.startswith("--") and a[2:] in ("focus", "key-down", "key-up", "ptt-on", "ptt-off", "cancel")]
    if cmds:
        ok = all(_remote(c) for c in cmds)
        return 0 if ok else 1
    app = Gtk.Application(application_id=APP_ID, flags=Gio.ApplicationFlags.NON_UNIQUE if os.environ.get("BRAINOS_FACE_MULTI") else Gio.ApplicationFlags.DEFAULT_FLAGS)
    app.connect("activate", lambda a: setattr(a, "face", Face(a)) if not hasattr(a, "face") else a.face.bar.win.present())
    return app.run([argv[0]])
