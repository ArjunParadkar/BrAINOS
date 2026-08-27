"""Operator ⇄ face link. Newline-delimited JSON over $XDG_RUNTIME_DIR/brainos/operator.sock.
Reader thread → GLib main loop callbacks; writer thread drains a queue. Never blocks the UI.
Reconnects every second forever."""
import json, os, queue, socket, threading, time
from gi.repository import GLib

def sock_path():
    p = os.environ.get("BRAINOS_SOCK")
    if p: return p
    rt = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    return os.path.join(rt, "brainos", "operator.sock")

class Link:
    def __init__(self, on_msg, on_conn):
        self.on_msg, self.on_conn = on_msg, on_conn
        self.path = sock_path()
        self.q = queue.Queue()
        self.sock = None; self.connected = False; self._stop = False
        threading.Thread(target=self._reader, daemon=True, name="ipc-reader").start()
        threading.Thread(target=self._writer, daemon=True, name="ipc-writer").start()

    def send(self, obj):
        self.q.put(json.dumps(obj, separators=(",", ":")) + "\n")

    def _set(self, up):
        if up != self.connected:
            self.connected = up
            GLib.idle_add(self.on_conn, up)

    def _reader(self):
        while not self._stop:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                s.connect(self.path)
            except OSError:
                s.close(); time.sleep(1.0); continue
            self.sock = s; self._set(True)
            buf = b""
            try:
                while True:
                    c = s.recv(1 << 16)
                    if not c: break
                    buf += c
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        line = line.strip()
                        if not line: continue
                        try: m = json.loads(line)
                        except ValueError: continue
                        GLib.idle_add(self.on_msg, m)
            except OSError:
                pass
            finally:
                self.sock = None; self._set(False)
                try: s.close()
                except OSError: pass
            time.sleep(1.0)

    def _writer(self):
        while not self._stop:
            line = self.q.get()
            for _ in range(50):           # hold outbound for up to ~5 s while reconnecting
                s = self.sock
                if s is not None:
                    try: s.sendall(line.encode()); break
                    except OSError: pass
                time.sleep(0.1)
