"""Markdown-lite → Pango markup (bold, italic, code, bullets, headings). Streaming-safe:
unclosed markers are left literal. Also a tiny HTML → Pango fallback for surfaces when
WebKit is absent (host testing only)."""
import html as _html, re

_B = re.compile(r"\*\*(.+?)\*\*")
_I = re.compile(r"(?<![\w*])\*(?!\*)([^*\n]+?)\*(?!\*)|(?<![\w_])_([^_\n]+?)_(?!\w)")
_C = re.compile(r"`([^`\n]+?)`")

def _inline(s, mono):
    s = _html.escape(s, quote=False)
    s = _C.sub(lambda m: f'<span font_family="{mono}" background="#1e1016" foreground="#ffb3d6"> {m.group(1)} </span>', s)
    s = _B.sub(r"<b>\1</b>", s)
    s = _I.sub(lambda m: f"<i>{m.group(1) or m.group(2)}</i>", s)
    return s

def md_to_pango(text, mono="monospace", accent="#ff4fa3", dim="#8d7a83"):
    out = []; in_code = False
    for line in text.split("\n"):
        if line.strip().startswith("```"):
            in_code = not in_code; continue
        if in_code:
            out.append(f'<span font_family="{mono}" foreground="#ffb3d6">{_html.escape(line, quote=False)}</span>'); continue
        m = re.match(r"^(#{1,3})\s+(.*)", line)
        if m:
            out.append(f'<span weight="bold" foreground="{accent}">{_inline(m.group(2), mono)}</span>'); continue
        m = re.match(r"^\s*([-*•]|\d+[.)])\s+(.*)", line)
        if m:
            bullet = "•" if not m.group(1)[0].isdigit() else m.group(1)
            out.append(f'<span foreground="{accent}">  {bullet} </span>{_inline(m.group(2), mono)}'); continue
        out.append(_inline(line, mono))
    return "\n".join(out)

_TAG = re.compile(r"<[^>]+>")
def html_to_text(h):
    h = re.sub(r"(?is)<(script|style)[^>]*>.*?</\1>", "", h)
    h = re.sub(r"(?i)<br\s*/?>|</p>|</div>|</h[1-6]>|</li>|</tr>", "\n", h)
    h = _TAG.sub("", h)
    h = _html.unescape(h)
    return re.sub(r"\n{3,}", "\n\n", h).strip()
