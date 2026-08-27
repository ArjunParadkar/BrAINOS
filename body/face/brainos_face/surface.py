"""Surfaces: named floating layer-shell panels on the right edge holding a WebKitGTK 6.0
WebView with operator-supplied HTML. `window.brainos.emit(event, data)` in the page →
{"t":"surface_event"} to the operator. Network is dead-ended (proxy to 127.0.0.1:9) so a
surface can never reach outside the body."""
import json
from gi.repository import Gtk, Gdk, GLib, Pango
from . import theme
from .markup import html_to_text

try:
    from gi.repository import Gtk4LayerShell as LS
except Exception:
    LS = None
try:
    import gi; gi.require_version("WebKit", "6.0")
    from gi.repository import WebKit
except Exception:
    WebKit = None

SIZES = {"small": (380, 320), "medium": (520, 640), "large": (760, 900)}
GAP = 12; TOP = 14; RIGHT = 14

BRIDGE_JS = r"""
(function(){
  window.brainos = {
    emit: function(event, data){
      try { window.webkit.messageHandlers.brainos.postMessage(JSON.stringify({event: String(event), data: (data===undefined?null:data)})); }
      catch(e) { console.error('brainos.emit failed', e); }
    }
  };
  document.addEventListener('DOMContentLoaded', function(){ if(!document.body.style.background && !document.body.style.backgroundColor) {} });
})();
"""
BASE_CSS = f"""
:root {{ color-scheme: dark; }}
html, body {{ margin:0; background: transparent; color: {theme.FG}; font-family: {theme.FONT}; font-size: 16px; }}
body {{ padding: 14px 16px; }}
a {{ color: {theme.ACCENT}; }}
button {{ background: {theme.ACCENT}; color: #1a0410; border: none; border-radius: 10px; padding: 8px 14px; font-weight: 700; font-size: 15px; cursor: pointer; }}
button:hover {{ filter: brightness(1.12); }}
input, textarea, select {{ background: rgba(255,79,163,.06); color: {theme.FG}; border: 1px solid rgba(255,79,163,.3); border-radius: 10px; padding: 8px 10px; font: inherit; }}
::-webkit-scrollbar {{ width: 8px; }} ::-webkit-scrollbar-thumb {{ background: rgba(255,79,163,.3); border-radius: 4px; }}
"""

class Surface:
    def __init__(self, mgr, name):
        self.mgr, self.name = mgr, name; self.size = "medium"
        w = self.win = Gtk.Window(); w.set_decorated(False); w.set_resizable(False)
        if LS:
            LS.init_for_window(w); LS.set_namespace(w, "brainos-surface")
            LS.set_layer(w, LS.Layer.TOP); LS.set_anchor(w, LS.Edge.RIGHT, True); LS.set_anchor(w, LS.Edge.TOP, True)
            LS.set_margin(w, LS.Edge.RIGHT, RIGHT); LS.set_keyboard_mode(w, LS.KeyboardMode.ON_DEMAND)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL); box.add_css_class("surface"); w.set_child(box)
        head = Gtk.Box(); head.add_css_class("surface-head")
        lbl = Gtk.Label(label=name.upper(), xalign=0, hexpand=True, ellipsize=Pango.EllipsizeMode.END); lbl.add_css_class("surface-name"); head.append(lbl)
        x = Gtk.Button(label="✕"); x.add_css_class("ghost"); x.connect("clicked", lambda *_: self.mgr.close(self.name, user=True)); head.append(x)
        box.append(head)
        self.view = None
        if WebKit:
            ucm = WebKit.UserContentManager()
            ucm.add_script(WebKit.UserScript.new(BRIDGE_JS, WebKit.UserContentInjectedFrames.ALL_FRAMES, WebKit.UserScriptInjectionTime.START, None, None))
            ucm.add_style_sheet(WebKit.UserStyleSheet.new(BASE_CSS, WebKit.UserContentInjectedFrames.ALL_FRAMES, WebKit.UserStyleLevel.USER, None, None))
            ucm.register_script_message_handler("brainos", None)
            ucm.connect("script-message-received::brainos", self._msg)
            ns = self.mgr.session()
            self.view = WebKit.WebView(user_content_manager=ucm, network_session=ns)
            s = self.view.get_settings(); s.set_enable_developer_extras(False); s.set_enable_javascript(True)
            s.set_default_font_size(16); s.set_enable_webgl(False)
            self.view.set_background_color(Gdk.RGBA(0, 0, 0, 0))
            self.view.connect("decide-policy", self._policy)
            self.view.set_vexpand(True); self.view.add_css_class("surface-body"); box.append(self.view)
        else:  # host without WebKitGTK 6.0: render the HTML as text so layout can still be verified
            self.fallback = Gtk.Label(xalign=0, yalign=0, wrap=True, vexpand=True, selectable=True); self.fallback.add_css_class("surface-fallback")
            sc = Gtk.ScrolledWindow(); sc.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC); sc.set_vexpand(True); sc.set_child(self.fallback); box.append(sc)
        w.present()

    def load(self, html, size=None):
        if size in SIZES: self.size = size
        wd, ht = SIZES[self.size]; self.win.set_default_size(wd, ht); self.win.set_size_request(wd, ht)
        if self.view: self.view.load_html(html, "brainos://surface/%s/" % self.name)
        else: self.fallback.set_text(html_to_text(html))

    def _msg(self, ucm, value):
        try:
            d = json.loads(value.to_string())
        except Exception:
            d = {"event": "message", "data": str(value.to_string())}
        self.mgr.send({"t": "surface_event", "name": self.name, "event": d.get("event", "event"), "data": d.get("data")})

    def _policy(self, view, decision, kind):
        # keep the page inside the body: allow only our own brainos:// / about: / data: navigations
        if kind in (WebKit.PolicyDecisionType.NAVIGATION_ACTION, WebKit.PolicyDecisionType.NEW_WINDOW_ACTION):
            uri = decision.get_navigation_action().get_request().get_uri() or ""
            if not (uri.startswith("brainos://") or uri.startswith("about:") or uri.startswith("data:")):
                decision.ignore(); return True
        return False

    def height(self):
        return SIZES[self.size][1] + 40  # header

    def set_top(self, y):
        if LS: LS.set_margin(self.win, LS.Edge.TOP, y)

class Surfaces:
    def __init__(self, send):
        self.send = send; self.items = {}; self._session = None
    def session(self):
        if self._session is None and WebKit:
            self._session = WebKit.NetworkSession.new_ephemeral()
            try:  # dead-end all network so surfaces never leave the body
                self._session.set_proxy_settings(WebKit.NetworkProxyMode.CUSTOM, WebKit.NetworkProxySettings.new("http://127.0.0.1:9", None))
            except Exception: pass
        return self._session
    def handle(self, m):
        name = str(m.get("name") or "surface")
        if m.get("close"): self.close(name); return
        s = self.items.get(name)
        if s is None: s = self.items[name] = Surface(self, name)
        s.load(m.get("html", "") or "", m.get("size")); self._tile()
    def close(self, name, user=False):
        s = self.items.pop(name, None)
        if s:
            s.win.destroy()
            if user: self.send({"t": "surface_event", "name": name, "event": "close", "data": None})
        self._tile()
    def _tile(self):
        y = TOP
        for s in self.items.values():
            s.set_top(y); y += s.height() + GAP
