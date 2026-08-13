#!/usr/bin/env python3
"""Tiny QMP helper for scripted BrAInOS demo/continuity testing.

Only touches the QEMU control plane (QMP socket) and the guest's emulated
keyboard/framebuffer via sendkey/screendump -- exactly what a human at the
GTK window would do with a real keyboard and their own eyes. No new limb,
no host bridge into the guest: this is test tooling on the outside of the
containment boundary, same category as the existing -chardev link socket.
"""
import argparse
import json
import socket
import sys
import time

_KEYMAP = {
    " ": "spc", "\n": "ret", "'": "apostrophe", "-": "minus", ".": "dot",
    ",": "comma", "?": "shift-slash", "!": "shift-1",
}


def key_for_char(c):
    if c in _KEYMAP:
        return _KEYMAP[c]
    if c.isalpha():
        return c.lower() if c.islower() else f"shift-{c.lower()}"
    if c.isdigit():
        return c
    return None


class QMP:
    def __init__(self, sock_path, timeout=10):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.settimeout(timeout)
        self.s.connect(sock_path)
        self.f = self.s.makefile("rwb")
        self._readline()  # greeting
        self._cmd({"execute": "qmp_capabilities"})

    def _readline(self):
        line = self.f.readline()
        if not line:
            raise ConnectionError("QMP socket closed")
        return json.loads(line)

    def _cmd(self, obj):
        self.f.write(json.dumps(obj).encode() + b"\n")
        self.f.flush()
        while True:
            r = self._readline()
            if "return" in r or "error" in r:
                return r
            # else: an async event line, keep reading

    def hmp(self, line):
        return self._cmd({"execute": "human-monitor-command",
                           "arguments": {"command-line": line}})

    def sendkey(self, key, hold_ms=60):
        parts = key.split("-")
        if len(parts) > 1:
            parts = [{"shift": "shift", "ctrl": "ctrl", "alt": "alt"}.get(p, p)
                     for p in parts]
        keys = [{"type": "qcode", "data": p} for p in parts]
        self._cmd({"execute": "send-key",
                   "arguments": {"keys": keys, "hold-time": hold_ms}})

    def type_text(self, text, delay=0.04):
        for c in text:
            k = key_for_char(c)
            if k is None:
                continue
            self.sendkey(k)
            time.sleep(delay)

    def press_enter(self):
        self.sendkey("ret")

    def press_esc(self):
        self.sendkey("esc")

    def screendump(self, path):
        return self.hmp(f"screendump {path}")

    def close(self):
        self.s.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("socket")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("type")
    p.add_argument("text")

    p = sub.add_parser("key")
    p.add_argument("qcode")

    p = sub.add_parser("screendump")
    p.add_argument("path")

    args = ap.parse_args()
    q = QMP(args.socket)
    if args.cmd == "type":
        q.type_text(args.text)
        q.press_enter()
    elif args.cmd == "key":
        q.sendkey(args.qcode)
    elif args.cmd == "screendump":
        q.screendump(args.path)
    q.close()


if __name__ == "__main__":
    sys.exit(main())
