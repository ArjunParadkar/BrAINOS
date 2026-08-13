#!/usr/bin/env python3
"""
world_test.py -- proof that the world disk (the entity's own file storage,
separate from the BrAIn Key) works end to end through the REAL core, with
no GPU and no LLM in the loop.

A scripted stand-in for the body daemon speaks the COM3 link protocol
directly, so every step is deterministic: it does the acquisition
handshake, then drives the core through four filesystem actions by feeding
it "heard" utterances and answering each think-request with a typed action
PROPOSAL (exactly what a mind would emit). The core runs each proposal
through KIRA and, because fs.* limbs live INSIDE the body, executes them
itself and hands back a grounded digest -- which this daemon captures off
the wire.

What it proves:
  1. the core discovers the world disk in real UEFI and incorporates the
     world/files limb (console: "incorporate world/files");
  2. fs.list reads a real directory (root, then DOCS);
  3. fs.read returns the real bytes of a seeded file;
  4. fs.write PERSISTS: a unique file written through the UEFI FAT driver
     is afterwards found physically in brainos-world.img, and reads back
     through the same limb;
  5. filesystem actions never cross the tether (zero AX on the wire) --
     they are genuinely local flesh, like the on-key notebook.

Usage: tools/world_test.py         (rebuilds nothing; boot + drive + verify)
Assumes ./build.sh (or an equivalent core build + key mint) has run and a
world disk exists; regenerates a fresh world disk so WORK/ starts empty.
"""
import os
import re
import socket
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from make_key import read_fat16_file  # noqa: E402
from qmp_client import QMP  # noqa: E402

QLOCAL = f"{ROOT}/tools/qemu-local"
KEY_IMG = f"{ROOT}/brainos-key.img"
WORLD_IMG = f"{ROOT}/brainos-world.img"
WORLD_PART_LBA = 2048  # single data partition, from make_world's GPT layout

SOCK = "/tmp/brainos_world_test.sock"
QMP_SOCK = "/tmp/brainos_world_test_qmp.sock"
CONSOLE_LOG = f"{ROOT}/workspace/logs/world_test_console.log"
VARS = "/tmp/brainos_world_test_vars.fd"

# Stage 2 close-out: the same suite must pass with the network organ
# present AND genuinely absent, because "honest refusal" is only proven by
# running the case where the capability really isn't there.
#   WORLD_TEST_NET=off  -> boot with -nic none; web.* must be refused
NET_ON = os.environ.get("WORLD_TEST_NET", "on") != "off"

GROUNDED_RE = re.compile(
    r"the real result was: (.*)\. tell the human what you", re.S
)


class ScriptedDaemon(threading.Thread):
    """The body daemon, minus the mind: a fixed script of typed actions.

    Owns all socket IO in its own thread. After the wake handshake it walks
    `actions` one at a time -- send a heard utterance, answer the resulting
    think-request with the action proposal, capture the grounded digest --
    recording the digest the core computed for each."""

    def __init__(self, actions, camera=False):
        super().__init__(daemon=True)
        self.camera = camera
        self.actions = actions          # [(heard, verb, target, kind), ...]
        self.results = [None] * len(actions)   # captured digests, in order
        self.idx = -1                   # which action we're mid-flight on
        self.awaiting = None            # None | "reply" | "grounded"
        # Which verbs crossed the tether. Internal limbs (fs/notes/ui/code/
        # web/app/screen) must NEVER appear here -- they are local flesh.
        # A TETHERED organ legitimately does: the camera lives on the
        # daemon side, exactly like the mic and speaker, so "zero AX" is
        # no longer the right invariant -- "no INTERNAL verb on the wire" is.
        self.ax_verbs = []
        self.done = threading.Event()
        self.err = None
        self._sock = None
        self._buf = b""
        self._lock = threading.Lock()
        self._timer = None

    def _send(self, line):
        with self._lock:
            self._sock.sendall(line if line.endswith(b"\n") else line + b"\n")

    def _answer_ax(self, verb, target):
        """Stand in for a tethered organ. The real daemon does the
        electrical work here; this scripts a deterministic result so the
        CORE side (KIRA grant -> AX -> AR -> StateNode) is what gets
        tested, with no GPU and no vision model in the loop."""
        if verb == "vision.look" and self.camera:
            return b"ARok|i can see a 320x240 frame: a test card at my lens"
        return b"ARerr|this body has no such limb"

    def _offer_limbs(self):
        # §8 acquisition: the audio jacks, so the core has a voice to speak
        # and treats injected utterances as heard sensory input.
        self._send(b"LM+mic|body/ears|sense.hearing|hears speech at the in jack")
        self._send(b"LM+speaker|body/voice|voice.speak|speaks through the out jack")
        if self.camera:
            # §8 acquisition of the lens, exactly as the real daemon offers it
            self._send(b"LM+camera|body/eyes|vision.look|sees frames at the video-in jack")

    def _start_next_action(self):
        if self._timer is not None:
            self._timer.cancel()
            self._timer = None
        self.idx += 1
        if self.idx >= len(self.actions):
            self.done.set()
            return
        heard = self.actions[self.idx][0]
        self.awaiting = "reply"
        self._send(b"HB" + heard.encode("ascii", "ignore"))

    def _handle_prompt(self, prompt):
        if prompt == "__hello__":
            self._send(b"MM!ready (scripted)")
            self._offer_limbs()
        elif prompt == "__wake__":
            self._send(b"MM!hello, i am here and listening.")
            self._start_next_action()          # kick off the script
        elif prompt.startswith("__grounded__"):
            m = GROUNDED_RE.search(prompt)
            if m and 0 <= self.idx < len(self.results):
                self.results[self.idx] = m.group(1).strip()
            self._send(b"MM!there we go.")       # close the turn, no action
            self._start_next_action()
        else:
            # a genuine think-request for a heard utterance: answer with the
            # scripted typed action proposal.
            if self.awaiting == "reply" and 0 <= self.idx < len(self.actions):
                _, verb, target, kind = self.actions[self.idx]
                self.awaiting = "grounded"
                body = f"on it~~{verb}|{target}".encode("ascii", "ignore")
                self._send(b"MM!" + body)
                if kind == "refuse":
                    # KIRA denies a limb the body lacks BEFORE any execution,
                    # so no __grounded__ will ever come back -- the core just
                    # speaks an honest refusal and returns to the loop. Advance
                    # on a timer so the script doesn't wedge waiting for it.
                    self._timer = threading.Timer(8.0, self._start_next_action)
                    self._timer.daemon = True
                    self._timer.start()
            else:
                self._send(b"MM!mm-hmm.")

    def run(self):
        try:
            for _ in range(150):             # up to ~15s for qemu to listen
                try:
                    self._sock = socket.socket(socket.AF_UNIX)
                    self._sock.connect(SOCK)
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    time.sleep(0.1)
            else:
                self.err = "could not connect to link socket"
                self.done.set()
                return
            self._sock.settimeout(420)
            while not self.done.is_set():
                try:
                    data = self._sock.recv(4096)
                except socket.timeout:
                    break
                if not data:
                    break
                self._buf += data
                while b"\n" in self._buf:
                    line, self._buf = self._buf.split(b"\n", 1)
                    line = line.strip()
                    if not line:
                        continue
                    if line.startswith(b"AX"):
                        verb, _, target = line[2:].decode(
                            "utf-8", "ignore").partition("|")
                        self.ax_verbs.append(verb.strip())
                        self._send(self._answer_ax(verb.strip(), target))
                    elif line.startswith(b"MM?"):
                        self._handle_prompt(line[3:].decode("utf-8", "ignore"))
                    # LA / other lines: acknowledged by ignoring, as the real
                    # daemon does.
        except Exception as e:            # noqa: BLE001 -- surface to main
            self.err = repr(e)
        finally:
            self.done.set()


def boot_qemu():
    for p in (SOCK, QMP_SOCK, CONSOLE_LOG):
        try:
            os.remove(p)
        except FileNotFoundError:
            pass
    os.makedirs(os.path.dirname(CONSOLE_LOG), exist_ok=True)
    import shutil
    shutil.copyfile(f"{QLOCAL}/usr/share/edk2/x64/OVMF_VARS.4m.fd", VARS)
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = f"{QLOCAL}/usr/lib"
    env["QEMU_MODULE_DIR"] = f"{QLOCAL}/usr/lib/qemu"
    cmd = [
        f"{QLOCAL}/usr/bin/qemu-system-x86_64",
        "-enable-kvm", "-m", "1G", "-cpu", "host",
        "-L", f"{QLOCAL}/usr/share/qemu",
        "-drive", f"if=pflash,format=raw,readonly=on,"
                  f"file={QLOCAL}/usr/share/edk2/x64/OVMF_CODE.4m.fd",
        "-drive", f"if=pflash,format=raw,file={VARS}",
        "-drive", f"format=raw,file={KEY_IMG}",
        # writethrough so a UEFI write is on the host image immediately, not
        # parked in qemu's writeback cache until shutdown.
        "-drive", f"format=raw,file={WORLD_IMG},cache=writethrough",
        "-chardev", f"socket,id=mm,path={SOCK},server=on,wait=off",
        "-device", "isa-serial,chardev=mm,index=2",
        "-serial", f"file:{CONSOLE_LOG}",
        "-qmp", f"unix:{QMP_SOCK},server,nowait",
        "-device", "qemu-xhci", "-device", "usb-tablet",
        # Stage 2.3: the entity's own network organ. User-mode slirp:
        # outbound only, no inbound, no host filesystem exposure. The
        # firmware's HTTP stack binds this at power-on.
        *(["-nic", "user,model=virtio-net-pci"] if NET_ON else ["-nic", "none"]),
        "-display", "none",
    ]
    return subprocess.Popen(cmd, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    codeword = f"cassiopeia{int(time.time()) % 100000}"
    wpath = f"WORK/NOTE.TXT"
    content = f"the world test left {codeword} here"

    fib = ("let a=0; let b=1; repeat 10 { let c=a+b; print c; "
           "let a=b; let b=c }")
    actions = [
        ("list the top of my disk please",   "fs.list", "",                    "exec"),
        ("now list the docs folder",          "fs.list", "DOCS",                "exec"),
        ("read docs slash about dot txt",     "fs.read", "DOCS/ABOUT.TXT",      "exec"),
        ("save a note to my work folder",     "fs.write", f"{wpath} {content}", "exec"),
        ("read my work note back to me",      "fs.read", wpath,                 "exec"),
        # the entity's own compute: an inline program on its silicon
        ("calculate six times seven",         "code.run",
         "let a=6; let b=7; print 'six times seven is', a*b",                  "exec"),
        # the write-then-run composition (§9.2/§13.1): a program becomes a
        # file, the file becomes a pathway
        ("write a fibonacci program to disk", "fs.write", f"WORK/FIB.BS {fib}", "exec"),
        ("run the program you saved",         "code.run", "WORK/FIB.BS",        "exec"),
        ("run it once more",                  "code.run", "WORK/FIB.BS",        "exec"),
        # self-presentation: the ambient presence, then back
        ("show yourself as the presence",     "ui.set",  "presence",            "exec"),
        ("back to the console please",        "ui.set",  "console",             "exec"),
        # ---- Stage 2.6: presentation as CONFIGURATION, not a toggle ----
        ("glow amber and go quiet",           "ui.set",  "presence amber quiet", "exec"),
        ("call yourself the night shift",     "ui.set",  "caption night shift", "exec"),
        # granted, but the limb itself fails: the core reports the failure
        # directly and sends no grounded digest, so advance on the timer
        ("that setting does not exist",       "ui.set",  "kaleidoscope",        "refuse"),
        ("console and speak up again",        "ui.set",  "console verbose",     "exec"),
        # ---- Stage 2.1: the filesystem grown to full competence ----
        ("make a sub folder in work",         "fs.mkdir", "WORK/SUB",           "exec"),
        ("write a shuttle file",              "fs.write",
         f"WORK/MOVEME.TXT shuttle payload {codeword}",                         "exec"),
        ("move it into the sub folder",       "fs.move",
         "WORK/MOVEME.TXT WORK/SUB/MOVED.TXT",                                  "exec"),
        ("read the moved file back",          "fs.read", "WORK/SUB/MOVED.TXT",  "exec"),
        ("how big is the moved file",         "fs.stat", "WORK/SUB/MOVED.TXT",  "exec"),
        ("find that codeword on my disk",     "fs.search", codeword,            "exec"),
        ("read the principles from byte forty", "fs.read", "DOCS/PRINCIPL.TXT@40", "exec"),
        ("write a temp file",                 "fs.write",
         "WORK/TEMP.TXT a disposable scratch line",                             "exec"),
        ("delete the temp file",              "fs.delete", "WORK/TEMP.TXT",     "exec"),
        # ---- Stage 2.5 (contained half): the screen as a SENSE. The heard
        # line below is rendered before the action runs, so finding it proves
        # the entity really reads its own display, not a canned string.
        ("say the word tangerine on screen",  "screen.read", "tangerine",       "exec"),
        ("what is on your screen now",        "screen.read", "",                "exec"),
        # ---- Stage 2.4: the expanded language, run on the entity's silicon.
        # A function, recursion, a while loop, a list and an indexed write --
        # a real (if small) program, not an arithmetic demo.
        ("write and run a real program",      "code.run",
         "fn fact(n) { if n <= 1 { return 1 }; return n * fact(n - 1) }; "
         "let xs = [3, 1, 2]; let i = 0; let sum = 0; "
         "while i < len(xs) { sum = sum + xs[i]; i = i + 1 }; xs[0] = 9; "
         "print \"fact5\", fact(5), \"sum\", sum, xs",                          "exec"),
        # ---- Stage 2.2: applications = the entity's OWN programs ----
        # The seeded one is discovered and runs with an argument...
        ("what programs do you have",         "app.list", "",                    "exec"),
        ("greet blur by name",                "app.run", "greet blur",           "exec"),
        # ...then it WRITES a new application and that one is discovered
        # too, which is the proof discovery is real and not hardcoded.
        # multi-line through a newline-framed tether: '\n' as two chars
        ("write yourself a squares program",  "fs.write",
         "PROGRAMS/SQUARES.BS # app: squares - print the first n squares"
         "\\nlet i = 1\\nwhile i <= args[0] { print i * i; i = i + 1 }",         "exec"),
        ("list your programs again",          "app.list", "",                    "exec"),
        ("run squares for four",              "app.run", "squares 4",            "exec"),
        ("run a program you don't have",      "app.run", "nosuchapp",            "refuse"),
        # ---- Stage 2.3: the internet, through this body's own organ ----
        ("look up example dot com",           "web.get", "http://example.com/",
         "exec" if NET_ON else "refuse"),
        ("save that page to my disk",         "web.save",
         "http://example.com/ WORK/PAGE.HTM",   "exec" if NET_ON else "refuse"),
        # KIRA's URL floor: shape, not reputation. Both are well-formed
        # proposals for a limb the body HAS -- they die at validate.
        ("fetch a malformed address",         "web.get", "not a url",            "refuse"),
        ("fetch with credentials in the url", "web.get",
         "http://user:pass@example.com/",                                        "refuse"),
        # ---- Stage 2.5: the lens, a TETHERED organ (like mic/speaker) ----
        ("what do you see right now",         "vision.look", "what do you see", "exec"),
        # structural self-harm: the volume marker is protected at POLICY —
        # the limb exists, the action is well-formed, KIRA still refuses.
        ("delete your volume marker",         "fs.delete", "WORLD.ID",          "refuse"),
        # honest refusal: a limb the body does not have. KIRA must DENY at
        # authz and the core must speak a refusal, never execute, never AX.
        ("search the web for the weather",    "web.search", "weather today",    "refuse"),
    ]
    refuse_idx = len(actions) - 1
    marker_idx = len(actions) - 2

    print("[world-test] regenerating a fresh world disk (WORK/ empty)...")
    subprocess.run([sys.executable, f"{HERE}/make_world.py", "--force", WORLD_IMG],
                   check=True, stdout=subprocess.DEVNULL)
    # sanity: the file we're about to write must NOT already exist
    assert read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK", "NOTE.TXT") is None, \
        "fresh world disk already has WORK/NOTE.TXT -- not a clean start"

    print("[world-test] booting core headless with the world disk...")
    daemon = ScriptedDaemon(actions, camera=True)
    qemu = boot_qemu()
    daemon.start()

    ok = daemon.done.wait(timeout=420)
    # give the console log a moment to flush the last lines
    time.sleep(1.0)

    # clean shutdown flushes everything to the image before we inspect it
    try:
        q = QMP(QMP_SOCK)
        q.hmp("quit")
        q.close()
    except Exception:
        qemu.terminate()
    try:
        qemu.wait(timeout=15)
    except subprocess.TimeoutExpired:
        qemu.kill()

    console = ""
    try:
        with open(CONSOLE_LOG, encoding="utf-8", errors="ignore") as f:
            console = f.read()
    except FileNotFoundError:
        pass

    failures = []

    def check(name, cond, detail=""):
        mark = "PASS" if cond else "FAIL"
        print(f"  [{mark}] {name}" + (f"  -- {detail}" if detail and not cond else ""))
        if not cond:
            failures.append(name)

    print("\n[world-test] results")
    if daemon.err:
        check("scripted daemon ran cleanly", False, daemon.err)
    check("script completed all actions", ok and daemon.idx >= len(actions),
          f"reached action {daemon.idx}/{len(actions)}")
    check("world/files limb incorporated at boot",
          "incorporate" in console and "world/files" in console)

    r = daemon.results
    # 1. root listing mentions the seeded top-level entries
    check("fs.list root sees seeded files",
          r[0] and "README.TXT" in r[0] and "DOCS" in r[0], f"got: {r[0]!r}")
    # 2. DOCS listing mentions the seeded docs
    check("fs.list DOCS sees ABOUT + PRINCIPL",
          r[1] and "ABOUT.TXT" in r[1] and "PRINCIPL.TXT" in r[1], f"got: {r[1]!r}")
    # 3. reading a seeded file returns its real content
    check("fs.read ABOUT.TXT returns real bytes",
          r[2] and "instance is to the device" in r[2], f"got: {r[2]!r}")
    # 4a. write reports bytes written
    check("fs.write reports a byte count",
          r[3] and "wrote" in r[3] and "NOTE.TXT" in r[3], f"got: {r[3]!r}")
    # 4b. THE persistence proof: the file is physically on the disk image
    on_disk = read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK", "NOTE.TXT")
    on_disk_txt = on_disk.decode("ascii", "ignore") if on_disk else ""
    check("written file is physically in brainos-world.img",
          on_disk is not None and codeword in on_disk_txt,
          f"on-disk: {on_disk_txt!r}")
    # 4c. and reads back through the limb
    check("fs.read of the new file returns what was written",
          r[4] and codeword in r[4], f"got: {r[4]!r}")
    # 5. the entity's own compute
    check("code.run inline: real arithmetic on the entity's silicon",
          r[5] and "six times seven is 42" in r[5], f"got: {r[5]!r}")
    check("fs.write stores the fibonacci program",
          r[6] and "wrote" in r[6] and "FIB.BS" in r[6], f"got: {r[6]!r}")
    check("code.run of the stored file: fibonacci output is right",
          r[7] and "89" in r[7] and "output:" in r[7], f"got: {r[7]!r}")
    check("second run still correct",
          r[8] and "89" in r[8], f"got: {r[8]!r}")
    # 6. §13.1 pathway learning, visible in the console
    check("pathway LEARNED on first write->run composition",
          "pathway learned: fs.write -> code.run" in console)
    check("pathway REUSED on the second run (cached route)",
          "pathway reused: fs.write -> code.run" in console)
    # 7. self-presentation is a real, KIRA-gated capability
    check("ui.set presence: entity switched to the ambient presence",
          r[9] is not None and "ambient presence" in r[9], f"got: {r[9]!r}")
    check("ui.set console: entity switched back",
          r[10] is not None and "console transcript" in r[10], f"got: {r[10]!r}")
    check("console restore visible on screen log",
          "console restored" in console)
    check("compute region incorporated at boot",
          "this-machine/compute" in console)
    # --- Stage 2.6: presentation as configuration, not a toggle ---
    check("ui.set applies palette + quiet + mode together",
          r[11] and "amber" in r[11] and "quiet" in r[11]
          and "ambient presence" in r[11], f"got: {r[11]!r}")
    check("ui.set takes a custom caption",
          r[12] and "night shift" in r[12], f"got: {r[12]!r}")
    check("an unknown setting fails honestly, options named on screen",
          "don't know that presentation setting" in console
          and "limb failed" in console)
    check("the failed setting produced no grounded result",
          r[13] is None, f"unexpected digest: {r[13]!r}")
    check("ui.set restores console and verbosity",
          r[14] and "console transcript" in r[14] and "showing again" in r[14],
          f"got: {r[14]!r}")
    # --- Stage 2.1: full filesystem competence ---
    check("fs.mkdir made WORK/SUB",
          r[15] and "made a folder" in r[15] and "SUB" in r[15], f"got: {r[15]!r}")
    check("fs.write staged the shuttle file",
          r[16] and "wrote" in r[16] and "MOVEME.TXT" in r[16], f"got: {r[16]!r}")
    check("fs.move reports the move",
          r[17] and "moved" in r[17] and "MOVED.TXT" in r[17], f"got: {r[17]!r}")
    moved = read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK/SUB", "MOVED.TXT")
    moved_txt = moved.decode("ascii", "ignore") if moved else ""
    check("moved file is physically at WORK/SUB/MOVED.TXT",
          moved is not None and codeword in moved_txt, f"on-disk: {moved_txt!r}")
    check("source of the move is physically gone",
          read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK", "MOVEME.TXT") is None)
    check("fs.read of the moved file returns the payload",
          r[18] and codeword in r[18], f"got: {r[18]!r}")
    check("fs.stat sees a file with a size",
          r[19] and "is a file" in r[19] and "bytes" in r[19], f"got: {r[19]!r}")
    check("fs.search finds the codeword by content",
          r[20] and ("MOVED.TXT" in r[20] or "NOTE.TXT" in r[20]), f"got: {r[20]!r}")
    check("chunked fs.read reports its byte window",
          r[21] and "from byte 40" in r[21], f"got: {r[21]!r}")
    check("fs.delete removed the temp file (digest)",
          r[23] and "gone" in r[23], f"got: {r[23]!r}")
    check("deleted file is physically gone from the image",
          read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK", "TEMP.TXT") is None)
    # --- Stage 2.5 (contained half): screen proprioception ---
    check("screen.read finds text the entity itself rendered",
          r[24] and "tangerine" in r[24].lower(), f"got: {r[24]!r}")
    check("screen.read blank reports the live display",
          r[25] and any(w in r[25].lower() for w in ("kira", "limb", "heard")),
          f"got: {r[25]!r}")
    # --- Stage 2.4: a real program through the real gate ---
    check("code.run: function, recursion, while loop, list write all correct",
          r[26] and "fact5 120" in r[26] and "sum 6" in r[26]
          and "[9, 1, 2]" in r[26], f"got: {r[26]!r}")
    # --- Stage 2.2: the entity's own applications, discovered ---
    check("programs region incorporated at boot",
          "this-machine/programs" in console)
    check("app.list discovers the seeded program and its description",
          r[27] and "greet" in r[27] and "hello" in r[27].lower(), f"got: {r[27]!r}")
    check("app.run passes arguments into the program",
          r[28] and "hello, blur" in r[28].lower(), f"got: {r[28]!r}")
    check("fs.write stored a NEW application",
          r[29] and "wrote" in r[29] and "SQUARES" in r[29], f"got: {r[29]!r}")
    check("the newly written program is DISCOVERED (not hardcoded)",
          r[30] and "squares" in r[30] and "greet" in r[30], f"got: {r[30]!r}")
    check("the newly written program RUNS correctly",
          r[31] and "1" in r[31] and "4" in r[31] and "9" in r[31]
          and "16" in r[31], f"got: {r[31]!r}")
    check("an unknown application fails honestly",
          "no program called" in console)
    check("unknown application produced no grounded result",
          r[32] is None, f"unexpected digest: {r[32]!r}")
    # --- Stage 2.3: the network organ, present or honestly absent ---
    saved = read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "WORK", "PAGE.HTM")
    if NET_ON:
        check("network organ incorporated with fetch capability",
              "this-machine/net" in console and "reach the internet" in console)
        check("boot probe reached the real internet",
              "HTTP 200" in console, "no HTTP 200 in the [net] verdict")
        check("web.get returns the page as MEANING, not bytes",
              r[33] and "Example Domain" in r[33], f"got: {r[33]!r}")
        check("web.get digest carries no html tags",
              r[33] and "<" not in r[33] and ">" not in r[33], f"got: {r[33]!r}")
        check("web.save reports bytes written",
              r[34] and "saved" in r[34] and "PAGE.HTM" in r[34], f"got: {r[34]!r}")
        check("downloaded page is physically on the world disk",
              saved is not None and b"<html" in (saved or b"").lower(),
              f"on-disk: {(saved or b'')[:60]!r}")
    else:
        # the honest-absence half: no organ, so the verb must be refused
        # and NOTHING may be fetched or written
        check("no network organ: probe says so plainly",
              "not applicable" in console or "no network organ" in console,
              "boot probe did not report the organ as absent")
        check("net region carries no fetch capability when absent",
              "reach the internet" not in console)
        check("web.get refused, no page returned",
              r[33] is None, f"unexpected digest: {r[33]!r}")
        check("web.save refused, nothing written",
              r[34] is None, f"unexpected digest: {r[34]!r}")
        check("no page reached the disk with the organ absent",
              saved is None, f"unexpected file on disk: {(saved or b'')[:40]!r}")
    check("malformed url denied at validate, never fetched",
          r[35] is None, f"unexpected digest: {r[35]!r}")
    check("credential-bearing url denied at validate",
          r[36] is None, f"unexpected digest: {r[36]!r}")
    # --- Stage 2.5: the lens, acquired over the tether and gated ---
    check("camera organ acquired via the §8 offer",
          "body/eyes" in console, "no eyes in the body map")
    check("vision.look granted and the real result absorbed",
          r[37] and "test card" in r[37], f"got: {r[37]!r}")
    check("vision.look DID cross the tether (it is a tethered organ)",
          "vision.look" in daemon.ax_verbs, f"ax verbs: {daemon.ax_verbs}")
    # the marker: KIRA must deny at POLICY (limb exists, action well-formed)
    check("WORLD.ID delete denied at policy, never executed",
          r[marker_idx] is None and "load-bearing" in console,
          f"digest: {r[marker_idx]!r}")
    check("volume marker is physically intact",
          (read_fat16_file(WORLD_IMG, WORLD_PART_LBA, "", "WORLD.ID") or b"")
          .startswith(b"BrAInOS world volume"))

    # 8. honest refusal + KIRA deny: the limbless action was refused, not run
    console_l = console.lower()
    kira_denied = ("deny" in console_l and "authz" in console_l) or \
                  "kira refused" in console_l or "no such limb" in console_l or \
                  "won't pretend" in console_l
    check("honest refusal: limbless action DENIED by KIRA + spoken refusal",
          kira_denied, "no KIRA deny / refusal text found in console")
    check("refused action produced NO grounded result (never executed)",
          r[refuse_idx] is None, f"unexpected digest: {r[refuse_idx]!r}")
    # 6. containment: fs (and the refused verb) never crossed the tether
    # The precise invariant: internal limbs are local flesh and must never
    # cross the tether, and a refused action must never be executed at all.
    # A tethered organ (the lens) legitimately does cross -- so assert on
    # WHICH verbs appeared, not that none did.
    internal_leaked = [v for v in daemon.ax_verbs
                       if v.startswith(("fs.", "notes.", "ui.", "code.",
                                        "web.", "app.", "screen."))]
    check("no INTERNAL verb ever crossed the tether (they are local flesh)",
          not internal_leaked, f"leaked: {internal_leaked}")
    check("refused verbs never reached the wire at all",
          "web.search" not in daemon.ax_verbs
          and "fs.delete" not in daemon.ax_verbs,
          f"ax verbs seen: {daemon.ax_verbs}")
    # boot self-test proves KIRA's deny path independently, every boot
    check("KIRA boot self-test present (memory.erase denied)",
          "memory.erase denied" in console_l)

    print()
    if failures:
        print(f"[world-test] FAILED: {len(failures)} check(s): {', '.join(failures)}")
        return 1
    print("[world-test] ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
