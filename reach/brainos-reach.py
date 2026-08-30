#!/usr/bin/env python3
"""
brainos-reach — offer this device's hands to a BrAInOS entity (Stage 4 §2).

One file. Python 3.8+ standard library only. Runs on Linux, macOS, Windows,
Termux (Android), Raspberry Pi. No pip.

    brainos-reach.py pair <entity public key hex>      # trust ONE entity
    brainos-reach.py serve --seat 192.168.1.10:7777 --name pi
    brainos-reach.py serve --seat auto --name phone    # find the seat by UDP beacon
    brainos-reach.py selftest                          # RFC 8032 vector

What this program is: a set of hands. It executes what the entity asks, over
one TCP connection, after the entity has proven — by an ed25519 signature
over a fresh nonce — that it holds the BrAIn Key this device was paired with.

What it is NOT: a memory. It keeps no history, no transcript, no result
cache. Every result goes back as a bounded digest and becomes memory on the
key (§13.2 memory-integrity law). The only files it writes are the pairing
(entity.pub) and a tamper log of refused handshakes.

Trust model, stated plainly: the signature is the authority; the transport is
the human's network. There is no TLS here. On a network you do not trust, run
it through a tunnel (WireGuard, ssh -L). Commands could be observed on a
hostile LAN; they cannot be forged, because every session is bound to a
challenge only the key can sign.
"""
import argparse
import hashlib
import json
import os
import platform
import secrets
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

VERSION = "4.0"
MAX_DIGEST = 4096
STATE_DIR = Path(os.environ.get("BRAINOS_REACH_DIR", Path.home() / ".brainos-reach"))
PAIR_FILE = STATE_DIR / "entity.pub"
TAMPER_LOG = STATE_DIR / "tamper.log"
BEACON_PORT = 7778

# ----------------------------------------------------------------------------
# ed25519 verification, RFC 8032, pure Python (after the reference
# implementation by Bernstein, Duif, Lange, Schwabe, Yang). Verification
# only — this device never signs anything.
# ----------------------------------------------------------------------------
_q = 2 ** 255 - 19
_l = 2 ** 252 + 27742317777372353535851937790883648493


def _inv(x):
    return pow(x, _q - 2, _q)


_d = (-121665 * _inv(121666)) % _q
_I = pow(2, (_q - 1) // 4, _q)


def _xrecover(y):
    xx = (y * y - 1) * _inv(_d * y * y + 1)
    x = pow(xx, (_q + 3) // 8, _q)
    if (x * x - xx) % _q != 0:
        x = (x * _I) % _q
    if x % 2 != 0:
        x = _q - x
    return x


_By = (4 * _inv(5)) % _q
_Bx = _xrecover(_By)
_B = (_Bx % _q, _By % _q)


def _edwards(P, Q):
    x1, y1 = P
    x2, y2 = Q
    x3 = (x1 * y2 + x2 * y1) * _inv(1 + _d * x1 * x2 * y1 * y2)
    y3 = (y1 * y2 + x1 * x2) * _inv(1 - _d * x1 * x2 * y1 * y2)
    return (x3 % _q, y3 % _q)


def _scalarmult(P, e):
    # iterative double-and-add (the reference is recursive; 512-bit scalars
    # would sit near Python's recursion limit)
    Q = (0, 1)
    for bit in bin(e)[2:]:
        Q = _edwards(Q, Q)
        if bit == "1":
            Q = _edwards(Q, P)
    return Q


def _encodepoint(P):
    x, y = P
    y_bits = y & ((1 << 255) - 1)
    return int(y_bits | ((x & 1) << 255)).to_bytes(32, "little")


def _decodepoint(s):
    n = int.from_bytes(s, "little")
    y = n & ((1 << 255) - 1)
    x = _xrecover(y)
    if (x & 1) != (n >> 255):
        x = _q - x
    P = (x, y)
    if (-x * x + y * y - 1 - _d * x * x * y * y) % _q != 0:
        raise ValueError("point not on curve")
    return P


def ed25519_verify(sig: bytes, msg: bytes, pk: bytes) -> bool:
    if len(sig) != 64 or len(pk) != 32:
        return False
    try:
        R = _decodepoint(sig[:32])
        A = _decodepoint(pk)
    except ValueError:
        return False
    S = int.from_bytes(sig[32:], "little")
    if S >= _l:
        return False
    h = int.from_bytes(hashlib.sha512(_encodepoint(R) + pk + msg).digest(), "little")
    return _scalarmult(_B, S) == _edwards(R, _scalarmult(A, h))


def selftest() -> bool:
    # RFC 8032 §7.1 test 1: empty message
    pk = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = bytes.fromhex(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    ok = ed25519_verify(sig, b"", pk)
    bad = ed25519_verify(sig, b"x", pk)
    t0 = time.time()
    ed25519_verify(sig, b"", pk)
    ms = (time.time() - t0) * 1000
    print(f"ed25519 verify: vector {'PASS' if ok else 'FAIL'}, tamper {'PASS' if not bad else 'FAIL'}, {ms:.0f} ms/verify")
    return ok and not bad


# ----------------------------------------------------------------------------
# hands — probed, only what exists on this device
# ----------------------------------------------------------------------------
OS = platform.system().lower()  # linux / darwin / windows
IS_TERMUX = "com.termux" in os.environ.get("PREFIX", "")


def which(*names):
    for n in names:
        if shutil.which(n):
            return n
    return None


def bounded(s: str) -> str:
    s = s if isinstance(s, str) else str(s)
    if len(s) > MAX_DIGEST:
        return s[:MAX_DIGEST] + "…[truncated]"
    return s


def run(argv, timeout=30, shell=False):
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout, shell=shell)
        out = (p.stdout or "") + (p.stderr or "")
        return bounded(out), p.returncode == 0
    except subprocess.TimeoutExpired:
        return f"timeout after {timeout}s", False
    except Exception as e:  # noqa: BLE001
        return f"exec error: {e}", False


class Hands:
    def __init__(self, home: Path):
        self.home = home.resolve()
        self.table = {}
        self._probe()

    def offers(self):
        return sorted(self.table)

    def _probe(self):
        t = self.table
        t["shell.run"] = self.shell_run
        t["code.run"] = self.code_run
        for v in ("fs.list", "fs.read", "fs.write", "fs.mkdir", "fs.delete", "fs.move", "fs.stat", "fs.search"):
            t[v] = getattr(self, v.replace(".", "_"))
        t["sys.time"] = lambda i: (time.strftime("%H:%M"), True)
        t["sys.date"] = lambda i: (time.strftime("%Y-%m-%d (%A)"), True)
        if which("notify-send", "osascript", "termux-notification", "powershell"):
            t["sys.notify"] = self.sys_notify
        if which("upower", "pmset", "termux-battery-status") or Path("/sys/class/power_supply").exists():
            t["sys.battery"] = self.sys_battery
        if which("wpctl", "amixer", "osascript", "termux-volume"):
            t["sys.volume"] = self.sys_volume
        if which("brightnessctl", "termux-brightness"):
            t["sys.brightness"] = self.sys_brightness
        if which("loginctl", "hyprlock", "swaylock", "pmset", "rundll32"):
            t["sys.lock"] = self.sys_lock
        if which("systemctl", "shutdown", "osascript"):
            t["sys.power"] = self.sys_power
        if which("wl-copy", "xclip", "pbcopy", "termux-clipboard-set", "clip"):
            t["sys.clipboard"] = self.sys_clipboard
        if which("xdg-open", "open", "termux-open", "gtk-launch") or OS == "windows":
            t["app.open"] = self.app_open
            t["app.list"] = self.app_list
        if which("grim", "scrot", "screencapture", "termux-camera-photo"):
            t["screen.look"] = self.screen_look
        if which("wtype", "xdotool", "osascript"):
            t["input.type"] = self.input_type
            t["input.key"] = self.input_key

    # -- shell / code --
    def shell_run(self, i):
        cmd = (i.get("cmd") or "").strip()
        if not cmd:
            return "error: no command", False
        timeout = min(int(i.get("timeout_s") or 30), 120)
        if OS == "windows":
            return run(cmd, timeout=timeout, shell=True)
        sh = which("bash", "sh") or "sh"
        return run([sh, "-c", cmd], timeout=timeout)

    def code_run(self, i):
        lang = (i.get("lang") or "python3").lower()
        src = i.get("src") or ""
        if not src:
            return "error: no source", False
        if lang.startswith("py"):
            return run([sys.executable, "-c", src], timeout=60)
        if lang in ("bash", "sh"):
            return run([which("bash", "sh") or "sh", "-c", src], timeout=60)
        if lang in ("node", "js") and which("node"):
            return run(["node", "-e", src], timeout=60)
        return f"error: no runtime for {lang} here", False

    # -- fs (under home) --
    def _p(self, s):
        p = Path(s or ".")
        p = p if p.is_absolute() else self.home / p
        p = p.resolve()
        if self.home not in p.parents and p != self.home:
            raise PermissionError(f"outside {self.home}")
        return p

    def fs_list(self, i):
        try:
            p = self._p(i.get("path"))
            names = []
            for c in sorted(p.iterdir())[:200]:
                names.append(c.name + ("/" if c.is_dir() else ""))
            return bounded("\n".join(names) or "(empty)"), True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_read(self, i):
        try:
            p = self._p(i.get("path"))
            off = int(i.get("offset") or 0)
            data = p.read_text(errors="replace")
            return bounded(data[off:off + MAX_DIGEST]), True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_write(self, i):
        try:
            p = self._p(i.get("path"))
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(i.get("content") or "")
            return f"wrote {p}", True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_mkdir(self, i):
        try:
            p = self._p(i.get("path"))
            p.mkdir(parents=True, exist_ok=True)
            return f"created {p}", True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_delete(self, i):
        try:
            p = self._p(i.get("path"))
            if p.is_dir():
                p.rmdir()
            else:
                p.unlink()
            return f"deleted {p}", True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_move(self, i):
        try:
            a = self._p(i.get("from") or i.get("path"))
            b = self._p(i.get("to"))
            a.rename(b)
            return f"moved to {b}", True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_stat(self, i):
        try:
            p = self._p(i.get("path"))
            st = p.stat()
            return f"{p}: {'dir' if p.is_dir() else 'file'}, {st.st_size} bytes, modified {time.strftime('%Y-%m-%d %H:%M', time.localtime(st.st_mtime))}", True
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False

    def fs_search(self, i):
        q = (i.get("query") or "").lower()
        if not q:
            return "error: empty query", False
        hits = []
        for root, dirs, files in os.walk(self.home):
            dirs[:] = [d for d in dirs if not d.startswith(".")][:50]
            for f in files:
                if q in f.lower():
                    hits.append(str(Path(root) / f))
                    if len(hits) >= 50:
                        return bounded("\n".join(hits)), True
        return bounded("\n".join(hits) or "no matches"), True

    # -- sys --
    def sys_notify(self, i):
        title, body = i.get("title") or "BrAInOS", i.get("body") or ""
        if which("notify-send"):
            return run(["notify-send", title, body])
        if which("termux-notification"):
            return run(["termux-notification", "--title", title, "--content", body])
        if which("osascript"):
            return run(["osascript", "-e", f'display notification "{body}" with title "{title}"'])
        if which("powershell"):
            return run(["powershell", "-Command", f"[console]::beep(); Write-Host '{title}: {body}'"])
        return "no notifier", False

    def sys_battery(self, i):
        if which("termux-battery-status"):
            return run(["termux-battery-status"])
        ps = Path("/sys/class/power_supply")
        if ps.exists():
            for b in ps.iterdir():
                cap = b / "capacity"
                if cap.exists():
                    return f"percentage: {cap.read_text().strip()}%", True
        if which("pmset"):
            return run(["pmset", "-g", "batt"])
        if which("upower"):
            return run(["bash", "-c", "upower -i $(upower -e | grep BAT | head -1) | grep -E 'percentage|state'"])
        return "no battery", False

    def sys_volume(self, i):
        a = (i.get("action") or "").lower()
        if which("wpctl"):
            m = {"up": ["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "5%+"], "down": ["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"], "mute": ["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]}
            return run(m.get(a) or ["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", f"{int(a or 50)}%"])
        if which("termux-volume"):
            return run(["termux-volume", "music", str(int(a) if a.isdigit() else 8)])
        if which("osascript"):
            v = {"up": "output volume of (get volume settings) + 10", "down": "output volume of (get volume settings) - 10", "mute": "output muted true"}.get(a, a or "50")
            return run(["osascript", "-e", f"set volume {v}"])
        return "no mixer", False

    def sys_brightness(self, i):
        a = (i.get("action") or "").lower()
        if which("brightnessctl"):
            return run(["brightnessctl", "set", {"up": "10%+", "down": "10%-"}.get(a, f"{a}%")])
        if which("termux-brightness"):
            return run(["termux-brightness", str(int(a) * 255 // 100 if a.isdigit() else 128)])
        return "no backlight control", False

    def sys_lock(self, i):
        for c in (["loginctl", "lock-session"], ["hyprlock"], ["swaylock"], ["pmset", "displaysleepnow"], ["rundll32", "user32.dll,LockWorkStation"]):
            if which(c[0]):
                subprocess.Popen(c)
                return "locked", True
        return "no locker", False

    def sys_power(self, i):
        a = (i.get("action") or "").lower()
        if a not in ("sleep", "shutdown", "reboot"):
            return "error: sleep|shutdown|reboot", False
        if which("systemctl"):
            return run(["systemctl", {"sleep": "suspend", "shutdown": "poweroff", "reboot": "reboot"}[a]])
        if which("osascript"):
            return run(["osascript", "-e", f'tell app "System Events" to {"sleep" if a == "sleep" else "shut down" if a == "shutdown" else "restart"}'])
        return "no power control", False

    def sys_clipboard(self, i):
        text = i.get("text") or i.get("content") or ""
        for c in (["wl-copy"], ["xclip", "-selection", "clipboard"], ["pbcopy"], ["termux-clipboard-set"], ["clip"]):
            if which(c[0]):
                try:
                    subprocess.run(c, input=text, text=True, timeout=5)
                    return "copied", True
                except Exception as e:  # noqa: BLE001
                    return f"error: {e}", False
        return "no clipboard", False

    # -- apps --
    def app_open(self, i):
        name = (i.get("name") or "").strip()
        if not name:
            return "error: no app", False
        if OS == "windows":
            return run(f'start "" "{name}"', shell=True)
        opener = which("gtk-launch", "xdg-open", "open", "termux-open")
        if opener == "gtk-launch":
            r = run(["gtk-launch", name])
            if r[1]:
                return r
            opener = which("xdg-open")
        if opener:
            subprocess.Popen([opener, name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return f"opened {name}", True
        return "no opener", False

    def app_list(self, i):
        d = Path("/usr/share/applications")
        if d.exists():
            return bounded("\n".join(sorted(p.stem for p in d.glob("*.desktop"))[:200])), True
        if OS == "darwin":
            return bounded("\n".join(sorted(p.stem for p in Path("/Applications").glob("*.app"))[:200])), True
        return "no application index here", False

    # -- screen / input --
    def screen_look(self, i):
        out = Path(os.environ.get("TMPDIR", "/tmp")) / f"brainos-reach-{int(time.time())}.png"
        for c in (["grim", str(out)], ["scrot", str(out)], ["screencapture", "-x", str(out)]):
            if which(c[0]):
                r, ok = run(c, timeout=15)
                return (f"screenshot saved on this device at {out} ({out.stat().st_size} bytes)" if ok and out.exists() else r), ok
        return "no screenshot tool", False

    def input_type(self, i):
        text = i.get("text") or ""
        if which("wtype"):
            return run(["wtype", text])
        if which("xdotool"):
            return run(["xdotool", "type", text])
        if which("osascript"):
            return run(["osascript", "-e", f'tell app "System Events" to keystroke "{text}"'])
        return "no typist", False

    def input_key(self, i):
        combo = i.get("combo") or ""
        if which("wtype"):
            args = []
            parts = combo.split("+")
            for m in parts[:-1]:
                args += ["-M", m]
            args += ["-k", parts[-1]]
            for m in reversed(parts[:-1]):
                args += ["-m", m]
            return run(["wtype"] + args)
        if which("xdotool"):
            return run(["xdotool", "key", combo])
        return "no key synth", False

    def call(self, verb, inp):
        fn = self.table.get(verb)
        if not fn:
            return f"error: this device has no hand for {verb}", False
        try:
            d, ok = fn(inp or {})
            return bounded(d), bool(ok)
        except Exception as e:  # noqa: BLE001
            return f"error: {e}", False


# ----------------------------------------------------------------------------
# pairing + tamper log
# ----------------------------------------------------------------------------
def load_pair():
    try:
        h = PAIR_FILE.read_text().strip()
        return bytes.fromhex(h) if len(h) == 64 else None
    except FileNotFoundError:
        return None


def tamper(reason, seat):
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    with TAMPER_LOG.open("a") as f:
        f.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {seat} {reason}\n")
    print(f"[reach] TAMPER: {reason} (from {seat})", file=sys.stderr)


# ----------------------------------------------------------------------------
# the session
# ----------------------------------------------------------------------------
def discover(timeout=3.0):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.settimeout(timeout)
    s.sendto(b"BRAINOS?", ("255.255.255.255", BEACON_PORT))
    try:
        data, (host, _) = s.recvfrom(128)
        parts = data.decode().split()
        if parts and parts[0] == "BRAINOS" and len(parts) >= 3:
            print(f"[reach] found seat '{parts[1]}' at {host}:{parts[2]}", file=sys.stderr)
            return f"{host}:{parts[2]}"
    except socket.timeout:
        pass
    return None


def serve_once(seat, name, hands, pinned, tofu):
    host, port = seat.rsplit(":", 1)
    sock = socket.create_connection((host, int(port)), timeout=10)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    f = sock.makefile("rwb", buffering=0)

    def send(o):
        f.write((json.dumps(o) + "\n").encode())

    def recv():
        line = f.readline()
        if not line:
            raise ConnectionError("seat closed the connection")
        return json.loads(line)

    nonce = secrets.token_hex(32)
    send({"t": "hello", "name": name, "nonce": nonce, "hands": hands.offers(),
          "os": ("termux" if IS_TERMUX else OS), "reach": VERSION})
    sock.settimeout(15)
    att = recv()
    if att.get("t") != "attest":
        raise ConnectionError(f"expected attest, got {att.get('t')}")
    try:
        sig = bytes.fromhex(att.get("sig", ""))
        pk = bytes.fromhex(att.get("pub", ""))
    except ValueError:
        tamper("malformed attestation", seat)
        send({"t": "refused", "reason": "malformed attestation"})
        return
    msg = f"{nonce}|{name}".encode()
    if pinned is None:
        if not tofu:
            send({"t": "refused", "reason": "not paired"})
            print(f"[reach] not paired. Run: brainos-reach.py pair {pk.hex()}  (fingerprint {pk.hex()[:8]}..{pk.hex()[-4:]})", file=sys.stderr)
            return
        pinned = pk
    if pk != pinned:
        tamper(f"key mismatch: offered {pk.hex()[:8]}.. pinned {pinned.hex()[:8]}..", seat)
        send({"t": "refused", "reason": "that is not my entity"})
        return
    if not ed25519_verify(sig, msg, pk):
        tamper("bad signature over challenge", seat)
        send({"t": "refused", "reason": "signature did not verify"})
        return
    if tofu and not PAIR_FILE.exists():
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        PAIR_FILE.write_text(pk.hex() + "\n")
        print(f"[reach] paired (TOFU) with {pk.hex()[:8]}..{pk.hex()[-4:]}", file=sys.stderr)
    send({"t": "ready"})
    print(f"[reach] '{name}' is now a body of {pk.hex()[:8]}.. ({len(hands.offers())} hands)", file=sys.stderr)

    sock.settimeout(None)
    while True:
        m = recv()
        t = m.get("t")
        if t == "cmd":
            digest, ok = hands.call(m.get("verb", ""), m.get("input") or {})
            send({"t": "result", "id": m.get("id"), "ok": ok, "digest": digest})
        elif t == "ping":
            send({"t": "pong"})
        elif t == "pong":
            pass
        elif t == "bye":
            print(f"[reach] seat said bye: {m.get('reason')}", file=sys.stderr)
            return


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pair", help="trust one entity by its public key hex")
    p.add_argument("pubhex")
    s = sub.add_parser("serve", help="offer this device's hands to the seat")
    s.add_argument("--seat", required=True, help="host:port of the seat, or 'auto'")
    s.add_argument("--name", required=True, help="how the entity will address this body (a-z0-9-)")
    s.add_argument("--home", default=str(Path.home()), help="root for fs.* hands")
    s.add_argument("--tofu", action="store_true", help="trust the first entity that attests correctly (prints its fingerprint)")
    s.add_argument("--once", action="store_true", help="do not reconnect")
    sub.add_parser("selftest")
    sub.add_parser("hands", help="print the hands this device would offer")
    a = ap.parse_args()

    if a.cmd == "selftest":
        sys.exit(0 if selftest() else 1)
    if a.cmd == "pair":
        if len(a.pubhex) != 64:
            sys.exit("public key must be 64 hex chars")
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        PAIR_FILE.write_text(a.pubhex.lower() + "\n")
        print(f"paired with {a.pubhex[:8]}..{a.pubhex[-4:]} → {PAIR_FILE}")
        return
    if a.cmd == "hands":
        print("\n".join(Hands(Path(a.home) if hasattr(a, "home") else Path.home()).offers()))
        return

    name = a.name.lower()
    if not name or len(name) > 32 or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-" for c in name) or name in ("seat", "local", "here"):
        sys.exit("name must be a-z, 0-9, '-' (not 'seat')")
    hands = Hands(Path(a.home))
    pinned = load_pair()
    if pinned is None and not a.tofu:
        sys.exit("not paired: run `brainos-reach.py pair <entity pub hex>` (from KEY.PUB on the key) or add --tofu")
    backoff = 1
    while True:
        seat = a.seat
        try:
            if seat == "auto":
                seat = discover() or ""
                if not seat:
                    raise ConnectionError("no seat answered the beacon")
            serve_once(seat, name, hands, pinned, a.tofu)
            pinned = load_pair() or pinned
            backoff = 1
        except (ConnectionError, OSError, json.JSONDecodeError) as e:
            print(f"[reach] {e}", file=sys.stderr)
        if a.once:
            return
        time.sleep(backoff)
        backoff = min(backoff * 2, 30)


if __name__ == "__main__":
    main()
