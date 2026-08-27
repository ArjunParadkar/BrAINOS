#!/usr/bin/env python3
"""Fake operator for exercising brainos-face on a dev host.

    BRAINOS_SOCK=/tmp/x/operator.sock python3 fake_operator.py [scene ...] [--gap 2.5] [--hold 20]

Listens on $BRAINOS_SOCK (default $XDG_RUNTIME_DIR/brainos/operator.sock), waits for the
face, then plays the named scenes (default: all) `--gap` seconds apart and keeps the socket
open `--hold` seconds after, echoing everything the face sends (utter/ptt/cancel/
surface_event/set_key — set_key is printed masked)."""
import json, os, socket, sys, time, threading

SOCK = os.environ.get("BRAINOS_SOCK") or os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "brainos", "operator.sock")

SURFACE_HTML = """<!doctype html><html><body>
<h2 style="margin:0 0 6px;color:#ff4fa3;letter-spacing:1px">Wi-Fi</h2>
<p style="color:#8d7a83;margin:0 0 14px">Pick a network. I will remember the password on the secure partition.</p>
<div id="nets"></div>
<p style="margin-top:16px"><input id="pw" type="password" placeholder="password" style="width:60%"> <button onclick="join()">Join</button></p>
<p id="out" style="color:#5cf0a5;font-family:monospace"></p>
<script>
const nets=[["Arenda-5G",92],["Paradkar Home",71],["Cafe Guest",38]];
const el=document.getElementById('nets');
nets.forEach(([n,s])=>{const d=document.createElement('div');
 d.style="display:flex;justify-content:space-between;align-items:center;padding:10px 12px;margin:6px 0;border:1px solid rgba(255,79,163,.25);border-radius:12px;cursor:pointer";
 d.innerHTML=`<span>${n}</span><span style="color:#8d7a83">${s}%</span>`;
 d.onclick=()=>{document.querySelectorAll('#nets div').forEach(x=>x.style.background='');d.style.background='rgba(255,79,163,.14)';window.sel=n;window.brainos.emit('select',{ssid:n});};
 el.appendChild(d);});
function join(){window.brainos.emit('click',{action:'join',ssid:window.sel||null,pw_len:document.getElementById('pw').value.length});document.getElementById('out').textContent='sent join → operator';}
</script></body></html>"""

def scenes(send):
    def state(mode, tier=None): send({"t": "state", "mode": mode, "tier": tier})
    S = {}
    def s_idle():  state("idle", None)
    def s_listen(): state("listening", None)
    def s_think(): state("thinking", "deliberate")
    def s_act():   state("acting", "reflex")
    def s_say():
        state("thinking", "deliberate"); time.sleep(0.6)
        text = ("Sure — here is the plan. I'll open **Firefox**, search for the *Arch wiki* page on `hyprland`, "
                "and pin it.\n\n- open the browser\n- run the search\n- pin the first result\n\n"
                "Say **cancel** at any point and I stop.")
        words = text.split(" ")
        for i, w in enumerate(words):
            send({"t": "say", "id": 7, "text": w + (" " if i < len(words) - 1 else ""), "delta": True, "done": False}); time.sleep(0.04)
        send({"t": "say", "id": 7, "text": "", "delta": True, "done": True}); state("idle", "deliberate")
    def s_acts():
        state("acting", "reflex")
        send({"t": "act", "verb": "app.open", "target": "firefox", "status": "grant", "digest": "KIRA grant UseLimb"}); time.sleep(0.5)
        send({"t": "act", "verb": "app.open", "target": "firefox", "status": "ok", "digest": "pid 4412"}); time.sleep(0.5)
        send({"t": "act", "verb": "web.search", "target": "arch wiki hyprland", "status": "ok", "digest": "12 results"}); time.sleep(0.5)
        send({"t": "act", "verb": "shell.run", "target": "rm -rf /", "status": "deny", "digest": "KIRA: destructive, outside /home/human"}); state("idle", "reflex")
    def s_surface(): send({"t": "surface", "name": "wifi", "html": SURFACE_HTML, "size": "medium"})
    def s_surface2(): send({"t": "surface", "name": "notes", "size": "small", "html": "<h3 style='color:#ff4fa3;margin:0'>Notes</h3><ul><li>Demo at 3 pm</li><li>Ship Stage 3</li></ul><button onclick=\"brainos.emit('click',{id:'add'})\">Add</button>"})
    def s_surface_close(): send({"t": "surface", "name": "wifi", "close": True}); send({"t": "surface", "name": "notes", "close": True})
    def s_teach(): send({"t": "teach", "x": 60, "y": 120, "w": 420, "h": 260, "caption": "This is your file manager. Click here to open your home folder."})
    def s_teach_clear(): send({"t": "teach", "clear": True})
    def s_task():
        steps = [{"title": "Open Firefox", "status": "done"}, {"title": "Search the Arch wiki for hyprland", "status": "running"}, {"title": "Pin the first result", "status": "pending"}]
        send({"t": "task", "id": 3, "title": "pin the hyprland wiki page", "steps": steps, "done": False}); state("acting", "deliberate")
    def s_task_done():
        steps = [{"title": "Open Firefox", "status": "done"}, {"title": "Search the Arch wiki for hyprland", "status": "done"}, {"title": "Pin the first result", "status": "failed"}]
        send({"t": "task", "id": 3, "title": "pin the hyprland wiki page", "steps": steps, "done": True}); state("idle", "deliberate")
    def s_need_key(): send({"t": "need_key"})
    for k, v in list(locals().items()):
        if k.startswith("s_"): S[k[2:]] = v
    return S

def main():
    args = sys.argv[1:]; gap = 2.5; hold = 30.0
    if "--gap" in args: i = args.index("--gap"); gap = float(args[i + 1]); del args[i:i + 2]
    if "--hold" in args: i = args.index("--hold"); hold = float(args[i + 1]); del args[i:i + 2]
    os.makedirs(os.path.dirname(SOCK), exist_ok=True)
    try: os.unlink(SOCK)
    except OSError: pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); srv.bind(SOCK); srv.listen(1)
    print(f"[fake-operator] listening {SOCK}", flush=True)
    conn, _ = srv.accept(); print("[fake-operator] face connected", flush=True)
    def send(o): conn.sendall((json.dumps(o) + "\n").encode())
    def reader():
        buf = b""
        while True:
            c = conn.recv(65536)
            if not c: print("[fake-operator] face hung up", flush=True); return
            buf += c
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                try: m = json.loads(line)
                except ValueError: continue
                if m.get("t") == "set_key": m["key"] = m["key"][:7] + "…(" + str(len(m["key"])) + " chars, masked)"
                print("[face→op]", json.dumps(m), flush=True)
    threading.Thread(target=reader, daemon=True).start()
    S = scenes(send)
    names = args or ["idle", "listen", "say", "acts", "task", "task_done", "surface", "surface2", "teach", "need_key"]
    for n in names:
        print(f"[fake-operator] scene {n}", flush=True); S[n](); time.sleep(gap)
    time.sleep(hold)

if __name__ == "__main__":
    main()
