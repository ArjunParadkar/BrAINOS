"""Committed dark brand look. Arenda pink accent; near-black warm ground.
Override any colour with BRAINOS_FACE_<NAME> env (hex) if a body build wants to."""
import os

def _e(name, default):
    v = os.environ.get("BRAINOS_FACE_" + name, "").strip()
    return v if v.startswith("#") and len(v) == 7 else default

BG      = _e("BG", "#0b0507")       # warm near-black ground
PANEL   = _e("PANEL", "#120a0e")
FG      = _e("FG", "#f6eef2")
DIM     = _e("DIM", "#8d7a83")
ACCENT  = _e("ACCENT", "#ff4fa3")   # Arenda pink
LISTEN  = _e("LISTEN", "#5fe0ff")
THINK   = _e("THINK", "#b78bff")
ACT     = _e("ACT", "#ffb54f")
OK      = _e("OK", "#5cf0a5")
ERR     = _e("ERR", "#ff4d5e")
FONT    = os.environ.get("BRAINOS_FACE_FONT", 'Inter, "JetBrainsMono Nerd Font", "Noto Sans", sans-serif')
MONO    = os.environ.get("BRAINOS_FACE_MONO", '"JetBrainsMono Nerd Font", "JetBrains Mono", monospace')

MODE_COLOR = {"idle": ACCENT, "listening": LISTEN, "thinking": THINK, "acting": ACT, "waking": DIM}

def rgb(h):
    h = h.lstrip("#"); return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))

def rgb255(h):
    h = h.lstrip("#"); return ",".join(str(int(h[i:i + 2], 16)) for i in (0, 2, 4))

def css():
    A, B, P = rgb255(ACCENT), rgb255(BG), rgb255(PANEL)
    return f"""
    window, window.background {{ background: transparent; }}
    * {{ font-family: {FONT}; }}
    .bar {{
        background-color: rgba({B}, 0.94);
        border: 1px solid rgba({A}, 0.38);
        border-radius: 20px;
        box-shadow: 0 8px 40px rgba(0,0,0,0.6), 0 0 0 1px rgba(0,0,0,0.5);
        color: {FG};
    }}
    .row {{ padding: 4px 10px 4px 8px; }}
    entry.cmd {{
        background: transparent; border: none; box-shadow: none; outline: none;
        color: {FG}; font-size: 17px; caret-color: {ACCENT}; min-height: 32px; padding: 0 6px;
    }}
    entry.cmd:focus, entry.cmd:focus-within {{ outline: none; box-shadow: none; }}
    entry.cmd placeholder, entry.cmd > text > placeholder {{ color: {DIM}; }}
    .status {{ color: {DIM}; font-size: 12px; letter-spacing: 1.5px; font-weight: 600; }}
    .status.live {{ color: {ACCENT}; }}
    .tier {{ color: {DIM}; font-size: 10px; letter-spacing: 2px; font-weight: 700; }}
    .reply {{ color: {FG}; font-size: 17px; line-height: 1.35; padding: 8px 14px 4px 14px; }}
    .reply-scroll {{ background: transparent; }}
    .heard {{ color: {DIM}; font-size: 15px; font-style: italic; padding: 8px 14px 0 14px; }}
    .ledger {{ padding: 4px 14px 6px 14px; }}
    .chip {{
        background: rgba({A}, 0.10); border: 1px solid rgba({A}, 0.22); border-radius: 9px;
        padding: 2px 9px; font-size: 12px; color: {FG}; font-family: {MONO};
    }}
    .chip.ok {{ border-color: rgba(92,240,165,0.35); }}
    .chip.deny, .chip.err {{ background: rgba(255,77,94,0.14); border-color: rgba(255,77,94,0.5); color: {ERR}; }}
    .chip.grant {{ border-color: rgba({A}, 0.5); }}
    .chip .verb {{ color: {ACCENT}; }}
    .chip.deny .verb, .chip.err .verb {{ color: {ERR}; }}
    .task {{ padding: 6px 14px 8px 14px; }}
    .task-title {{ color: {DIM}; font-size: 11px; letter-spacing: 2px; font-weight: 700; }}
    .task-title.done {{ color: {OK}; }}
    .step {{ font-size: 14px; color: {DIM}; padding: 2px 0; }}
    .step.running {{ color: {FG}; }}
    .step.done {{ color: {FG}; }}
    .step.failed {{ color: {ERR}; }}
    .step .glyph {{ font-family: {MONO}; font-size: 14px; min-width: 22px; }}
    .step.pending .glyph {{ color: {DIM}; }}
    .step.running .glyph {{ color: {ACCENT}; }}
    .step.done .glyph {{ color: {OK}; }}
    .step.failed .glyph {{ color: {ERR}; }}
    .sep {{ background: rgba({A}, 0.16); min-height: 1px; margin: 0 12px; }}
    .keybox {{ padding: 10px 14px 8px 14px; }}
    .keybox .h {{ color: {FG}; font-size: 15px; font-weight: 600; }}
    .keybox .p {{ color: {DIM}; font-size: 12px; }}
    .keybox entry:focus-within {{ outline: 2px solid rgba({A}, 0.55); outline-offset: -1px; border-color: {ACCENT}; }}
    .keybox entry {{
        background: rgba({A}, 0.06); border: 1px solid rgba({A}, 0.3); border-radius: 10px;
        color: {FG}; font-family: {MONO}; font-size: 14px; padding: 4px 10px; min-height: 30px;
    }}
    button.pink {{
        background: {ACCENT}; color: #1a0410; border: none; border-radius: 10px;
        font-weight: 700; padding: 4px 14px; min-height: 30px; box-shadow: none;
    }}
    button.pink:hover {{ background: #ff6fb6; }}
    button.ghost {{ background: transparent; color: {DIM}; border: none; box-shadow: none; padding: 2px 8px; min-height: 0; }}
    button.ghost:hover {{ color: {FG}; }}

    .surface {{
        background-color: rgba({P}, 0.97);
        border: 1px solid rgba({A}, 0.35); border-radius: 16px;
        box-shadow: 0 10px 40px rgba(0,0,0,0.65);
        color: {FG};
    }}
    .surface-head {{ padding: 6px 8px 6px 14px; }}
    .surface-name {{ color: {ACCENT}; font-size: 12px; letter-spacing: 2px; font-weight: 700; }}
    .surface-body {{ background: transparent; }}
    .surface-fallback {{ color: {FG}; font-size: 14px; padding: 12px 16px; }}
    """
