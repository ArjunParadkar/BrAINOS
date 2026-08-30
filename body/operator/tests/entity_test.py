#!/usr/bin/env python3
"""
entity_test.py — Stage 4 live harness (docs/STAGE4_ENTITY.md §7).

Starts a real brainos-operator on this host, a real `brainos-reach.py` body
on localhost, drives the operator over its face socket and checks:

  T0  reflex latency on the seat ("what time is it")
  T1  Reach handshake: a body paired with the entity's key joins; a body
      paired with a WRONG key is refused and logs a tamper event
  T2  "on lab, what time is it" — a reflex intent routed to the Reach body
  T3  a deliberate turn that uses a hand on the Reach body is COMPILED into a
      skill; the same utterance again replays in the skill tier (both timed)
  T8  the operator is restarted: the signed skill store rehydrates and the
      utterance still replays (persistence across lives)
  T4  "forget that" drops the skill; the utterance goes back to thinking
  T5  a watcher (task.when) set up by the deliberate tier fires when its
      predicate comes true
  T6  the Reach body disconnects: the face is told, body.list no longer
      lists it, and the routed form no longer reaches it
  T7  speculative cognition: with no local model, the deliberate first token
      must arrive within a bounded time of the utterance

Usage:
  ANTHROPIC_API_KEY=$(cat ~/.brainos_api_key) python3 tests/entity_test.py
Without a key T3/T4/T5/T7/T8 are skipped (the rest still run).
"""
import json
import os
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
OP_DIR = HERE.parent
REPO = OP_DIR.parent.parent
BIN = OP_DIR / "target" / "release" / "brainos-operator"
REACH = REPO / "reach" / "brainos-reach.py"
SCRATCH = Path(os.environ.get("ENTITY_SCRATCH", f"/tmp/brainos-entity-{os.getpid()}"))
SOCK = SCRATCH / "operator.sock"
REACH_ADDR = "127.0.0.1:7791"
API = os.environ.get("ANTHROPIC_API_KEY", "")

results = []


def ok(name, cond, detail=""):
    results.append((name, bool(cond), detail))
    print(f"  {'PASS' if cond else 'FAIL'} {name}{(' — ' + detail) if detail else ''}", flush=True)
    return bool(cond)


def skip(name, why):
    results.append((name, None, why))
    print(f"  SKIP {name} — {why}", flush=True)


# ---------------------------------------------------------------- setup
def setup_brain():
    brain = SCRATCH / "brain"
    brain.mkdir(parents=True, exist_ok=True)
    k = json.loads((REPO / "key" / "brain_key.json").read_text())
    (brain / "SEED.HEX").write_text(k["private_seed"] + "\n")
    (brain / "KEY.PUB").write_text(k["public_key"] + "\n")
    return brain, k["public_key"]


def write_config(brain):
    cfg = SCRATCH / "operator.toml"
    cfg.write_text(f"""
[operator]
socket_path = "{SOCK}"
brain_dir = "{brain}"

[anthropic]
model = "claude-fable-5"
effort = "low"
key_device = ""

[llama]
url = "http://127.0.0.1:1"      # nothing listens: the fast tier must fail fast

[reach]
listen = "{REACH_ADDR}"
beacon_port = 0
seat_name = "testseat"

[tiers]
speculate = true
fast_budget_ms = 600

[task]
scratchpad = "{SCRATCH}/task"
""")
    return cfg


def start_operator(cfg, log_name):
    env = dict(os.environ)
    env["RUST_LOG"] = "brainos_operator=info"
    env.pop("BRAINOS_SOCK", None)
    env.pop("BRAINOS_REACH", None)
    if API:
        env["ANTHROPIC_API_KEY"] = API
    else:
        env.pop("ANTHROPIC_API_KEY", None)
    log = open(SCRATCH / log_name, "w")
    p = subprocess.Popen([str(BIN), "--config", str(cfg)], stderr=log, stdout=log, env=env, cwd=str(SCRATCH))
    for _ in range(100):
        if SOCK.exists():
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(str(SOCK))
                s.close()
                time.sleep(0.2)
                return p
            except OSError:
                pass
        time.sleep(0.1)
    raise RuntimeError("operator did not come up")


def start_reach(name, pub, home, tofu=False):
    d = SCRATCH / f"reach-{name}"
    d.mkdir(parents=True, exist_ok=True)
    home.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, BRAINOS_REACH_DIR=str(d))
    subprocess.run([sys.executable, str(REACH), "pair", pub], env=env, check=True, capture_output=True)
    log = open(d / "log", "w")
    args = [sys.executable, str(REACH), "serve", "--seat", REACH_ADDR, "--name", name, "--home", str(home)]
    p = subprocess.Popen(args, env=env, stderr=log, stdout=log)
    return p, d


class Face:
    def __init__(self):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(str(SOCK))
        self.buf = b""
        self.backlog = []

    def send(self, o):
        self.s.sendall((json.dumps(o) + "\n").encode())

    def _read(self, timeout):
        self.s.settimeout(timeout)
        try:
            chunk = self.s.recv(65536)
        except socket.timeout:
            return []
        if not chunk:
            raise ConnectionError("operator closed")
        self.buf += chunk
        out = []
        while b"\n" in self.buf:
            line, self.buf = self.buf.split(b"\n", 1)
            if line.strip():
                try:
                    out.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return out

    def drain(self, timeout=0.3):
        got = []
        end = time.time() + timeout
        while time.time() < end:
            got += self._read(max(0.05, end - time.time()))
        return got

    def utter(self, text, timeout=90):
        """Send an utterance; collect until state:idle. Returns (msgs, first_say_ms, total_ms)."""
        self.drain(0.2)
        t0 = time.time()
        self.send({"t": "utter", "text": text})
        msgs, first = [], None
        end = time.time() + timeout
        while time.time() < end:
            for m in self._read(0.5):
                msgs.append(m)
                if first is None and m.get("t") == "say" and m.get("text"):
                    first = (time.time() - t0) * 1000
                if m.get("t") == "state" and m.get("mode") == "idle":
                    return msgs, first, (time.time() - t0) * 1000
        return msgs, first, (time.time() - t0) * 1000

    def wait_for(self, pred, timeout):
        end = time.time() + timeout
        while time.time() < end:
            for m in self._read(0.5):
                if pred(m):
                    return m
        return None


def says(msgs):
    return " ".join(m.get("text", "") for m in msgs if m.get("t") == "say" and m.get("text")).strip()


def acts(msgs, verb=None):
    return [m for m in msgs if m.get("t") == "act" and (verb is None or m.get("verb") == verb)]


# ---------------------------------------------------------------- tests
def main():
    if not BIN.exists():
        sys.exit(f"build first: cargo build --release ({BIN} missing)")
    SCRATCH.mkdir(parents=True, exist_ok=True)
    brain, pub = setup_brain()
    cfg = write_config(brain)
    print(f"scratch: {SCRATCH}\nentity pub: {pub[:8]}..{pub[-4:]}\napi key: {'yes' if API else 'NO (deliberate tests skipped)'}", flush=True)

    procs = []
    op = start_operator(cfg, "operator.log")
    procs.append(op)
    face = Face()
    face.drain(0.5)

    try:
        # ---- T0 reflex on the seat
        print("\n[T0] reflex on the seat", flush=True)
        msgs, first, total = face.utter("what time is it", timeout=10)
        ok("T0 reflex sys.time", any(c.isdigit() for c in says(msgs)), f"{says(msgs)!r} total {total:.1f} ms")

        # ---- T1 reach handshake (good key) + refusal (wrong key)
        print("\n[T1] Reach handshake", flush=True)
        labhome = SCRATCH / "labhome"
        lab, labdir = start_reach("lab", pub, labhome)
        procs.append(lab)
        joined = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "lab" and m.get("joined"), 20)
        ok("T1a body 'lab' joined", joined is not None, f"hands={len(joined.get('hands', [])) if joined else 0} os={joined.get('os') if joined else '?'} rtt={joined.get('rtt_ms') if joined else '?'}ms")

        wrong = "00" * 32
        evil, evildir = start_reach("evil", wrong, SCRATCH / "evilhome")
        procs.append(evil)
        t_end = time.time() + 15
        tlog = evildir / "tamper.log"
        while time.time() < t_end and not tlog.exists():
            time.sleep(0.3)
        ok("T1b wrong-key body refused + tamper logged", tlog.exists(), (tlog.read_text().strip().splitlines()[-1] if tlog.exists() else "no tamper.log"))
        evil.send_signal(signal.SIGTERM)
        never = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "evil", 2)
        ok("T1c 'evil' never incorporated", never is None)

        # ---- T2 reflex routed to the body
        print("\n[T2] reflex routed over Reach", flush=True)
        msgs, first, total = face.utter("on lab, what time is it", timeout=15)
        a = acts(msgs, "sys.time")
        ok("T2 sys.time ran on 'lab'", a and a[0].get("body") == "lab" and a[0].get("status") == "ok", f"act={a[0] if a else None} total {total:.1f} ms")

        # ---- T3 deliberate → skill → replay
        print("\n[T3] deliberate compiles a skill; replay is reflex-speed", flush=True)
        (labhome / "hello.txt").write_text("reach test file\n")
        utt = f"on lab, list the files in {labhome}"
        if not API:
            skip("T3", "no ANTHROPIC_API_KEY")
            skip("T8", "no ANTHROPIC_API_KEY")
            skip("T4", "no ANTHROPIC_API_KEY")
            skip("T5", "no ANTHROPIC_API_KEY")
            skip("T7", "no ANTHROPIC_API_KEY")
        else:
            msgs, first, total_d = face.utter(utt, timeout=120)
            fa = [m for m in acts(msgs, "fs.list") if m.get("tier") == "deliberate"]
            ok("T3a deliberate fs.list on 'lab'", fa and fa[0].get("body") == "lab" and fa[0].get("status") == "ok",
               f"first token {first:.0f} ms, total {total_d:.0f} ms, act={fa[0] if fa else None}")
            saw = "hello.txt" in json.dumps(msgs)
            ok("T3b the body's real file came back", saw)
            time.sleep(0.3)
            msgs2, first2, total_r = face.utter(utt, timeout=15)
            sa = [m for m in acts(msgs2, "fs.list") if m.get("tier") == "skill"]
            ok("T3c replay hit the skill tier", sa and sa[0].get("body") == "lab" and sa[0].get("status") == "ok",
               f"total {total_r:.1f} ms (deliberate was {total_d:.0f} ms → {total_d / max(total_r, 0.01):.0f}x)")
            ok("T3d replay under 50 ms + RTT", total_r < 50 + 100, f"{total_r:.1f} ms")
            ok("T3e replay returned the file", "hello.txt" in says(msgs2))

            # ---- T8 persistence across lives
            print("\n[T8] restart: signed skills rehydrate", flush=True)
            op.send_signal(signal.SIGINT)
            op.wait(timeout=10)
            skl = list(brain.glob("SKL_*"))
            ok("T8a SKL_A/B written on the key", bool(skl), ", ".join(p.name for p in skl))
            op = start_operator(cfg, "operator2.log")
            procs.append(op)
            face = Face()
            face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "lab" and m.get("joined"), 40)  # reach reconnects with backoff
            msgs3, _, total_r2 = face.utter(utt, timeout=15)
            sa3 = [m for m in acts(msgs3, "fs.list") if m.get("tier") == "skill"]
            ok("T8b utterance replays after restart", bool(sa3), f"total {total_r2:.1f} ms")
            log2 = (SCRATCH / "operator2.log").read_text()
            ok("T8c loader reports the skill", "[skills] loaded 1 skills" in log2)

            # ---- T4 forget
            print("\n[T4] forget that", flush=True)
            msgs4, _, _ = face.utter("forget that", timeout=10)
            ok("T4a forgotten", "forgotten" in says(msgs4).lower(), says(msgs4)[:80])
            msgs5, _, total5 = face.utter(utt, timeout=120)
            ok("T4b back to thinking (no skill-tier act)", not any(m.get("tier") == "skill" for m in acts(msgs5)) and bool(acts(msgs5, "fs.list")), f"total {total5:.0f} ms")

            # ---- T5 watcher
            print("\n[T5] standing watcher", flush=True)
            flag = SCRATCH / "flag"
            msgs6, _, _ = face.utter(
                f"set up a watcher for me: run the shell command `ls {SCRATCH}` every 15 seconds, and when its output contains the word flag, do this once: note: the watcher fired",
                timeout=120)
            tw = acts(msgs6, "task.when")
            ok("T5a task.when created", tw and tw[0].get("status") == "ok", f"{tw[0].get('digest') if tw else says(msgs6)[:100]}")
            if tw:
                flag.write_text("x")
                fired = face.wait_for(lambda m: m.get("t") == "heard" and "standing task" in m.get("text", ""), 60)
                ok("T5b watcher fired within 60 s", fired is not None, fired.get("text") if fired else "")
                noted = face.wait_for(lambda m: m.get("t") == "act" and m.get("verb") == "notes.write", 20)
                ok("T5c its utterance ran (notes.write)", noted is not None)
                msgs7, _, _ = face.utter("what are you watching", timeout=10)
                ok("T5d task.list shows it done", "done" in says(msgs7) or "no standing tasks" in says(msgs7), says(msgs7)[:100])

            # ---- T7 speculative first token
            print("\n[T7] speculative deliberate (fast tier absent)", flush=True)
            msgs8, first8, total8 = face.utter("in one short sentence, what is a BrAIn Key?", timeout=90)
            ok("T7 first deliberate token within 8 s", first8 is not None and first8 < 8000, f"first {first8:.0f} ms, total {total8:.0f} ms, reply {says(msgs8)[:80]!r}")
            log = (SCRATCH / ("operator2.log" if (SCRATCH / "operator2.log").exists() else "operator.log")).read_text()
            ok("T7b speculative path taken", "(speculative)" in log)

        # ---- T6 disconnect
        print("\n[T6] body leaves", flush=True)
        lab.send_signal(signal.SIGTERM)
        left = face.wait_for(lambda m: m.get("t") == "body" and m.get("name") == "lab" and not m.get("joined"), 15)
        ok("T6a face told 'lab' left", left is not None)
        msgs9, _, _ = face.utter("where are you", timeout=10)
        ok("T6b body.list no longer lists lab", "lab:" not in says(msgs9) and "seat" in says(msgs9), says(msgs9)[:80])
        msgs10, _, _ = face.utter("on lab, what time is it", timeout=60)
        ok("T6c nothing reaches lab any more", not any(m.get("body") == "lab" for m in acts(msgs10)))

    finally:
        for p in procs:
            try:
                p.send_signal(signal.SIGTERM)
            except Exception:
                pass
        time.sleep(0.5)
        for p in procs:
            try:
                p.kill()
            except Exception:
                pass

    print("\n=== SUMMARY ===")
    fails = [r for r in results if r[1] is False]
    skips = [r for r in results if r[1] is None]
    print(f"  {len([r for r in results if r[1]])} passed, {len(fails)} failed, {len(skips)} skipped")
    for n, _, d in fails:
        print(f"  FAIL {n}: {d}")
    print(f"  logs: {SCRATCH}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
