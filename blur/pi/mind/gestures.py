"""
gestures.py -- the gesture planner: BLUR's body language, synced with speech.

Maps dialogue acts and emotion to gestures on the actuators she actually has
(neck lift/pan/tilt, jaw, the two tail winches -- there are no ear servos, so
"ear" reactions are rendered as small head/tail flicks and labelled as such).
Every gesture is KIRA-gated, like any other body skill, and is emitted as
joint setpoints so it drives the MuJoCo twin's neck/jaw/tail and the laptop
page before the robot exists (sim-first).

The thinking display (head tilt + gaze aversion up-and-to-the-side + a small
flick + a soft "hmm") is itself a gesture, requested through KIRA when the
slow path runs.
"""
from __future__ import annotations

from dataclasses import dataclass, field

# neutral/rest neck+tail pose (rad); gestures are deltas toward target poses
REST = {"neck_lift": 0.0, "neck_pan": 0.0, "neck_tilt": 0.0, "jaw": 0.0,
        "tail_winch_pitch": 0.0, "tail_winch_yaw": 0.0}


@dataclass
class Gesture:
    name: str
    targets: dict               # actuator -> target angle (rad)
    duration_s: float = 0.6
    oscillate: bool = False     # True = wag/nod back-and-forth
    note: str = ""


class GesturePlanner:
    """Produces gestures; `emit` gates each through KIRA and sends to a sink."""
    def __init__(self, gate_check, sink=None):
        self.gate_check = gate_check        # (skill_name) -> (verdict, reason)
        self.sink = sink or ConsoleSink()
        self.emitted = []

    # --- primitive gestures ---
    def gaze(self, cx=0.0, cy=0.0):
        return Gesture("gaze", {"neck_pan": -0.8 * cx, "neck_tilt": -0.5 * cy}, 0.3)

    def nod(self):
        return Gesture("nod", {"neck_tilt": 0.18}, 0.4, oscillate=True)

    def perk_up(self):
        return Gesture("perk_up", {"neck_lift": 0.15, "neck_pan": 0.05}, 0.3,
                       note="ears-forward rendered as a small head lift (no ear servos)")

    def head_tilt(self):
        return Gesture("head_tilt", {"neck_tilt": 0.25, "neck_pan": 0.12}, 0.5)

    def thinking(self):
        return Gesture("think", {"neck_tilt": 0.22, "neck_pan": 0.35, "neck_lift": 0.1},
                       0.8, note="gaze up-and-to-the-side + head tilt + flick (thinking display)")

    def tail_wag(self):
        return Gesture("tail_wag", {"tail_winch_yaw": 0.2}, 0.8, oscillate=True)

    def tail_low(self):
        return Gesture("tail_low", {"tail_winch_pitch": -0.3}, 0.6)

    def jaw_talk(self, open_frac=0.5):
        return Gesture("jaw", {"jaw": 0.25 * open_frac}, 0.15, oscillate=True)

    # --- dialogue act + emotion -> gestures ---
    def for_dialogue_act(self, act: str, emotion: str = "warm"):
        g = []
        if act == "greeting":
            g += [self.perk_up(), self.tail_wag()]
        elif act == "affirm":
            g += [self.nod()]
        elif act == "question":
            g += [self.head_tilt()]
        elif act == "thinking":
            g += [self.thinking()]
        if emotion == "bright":
            g += [self.tail_wag()]
        elif emotion in ("low",):
            g += [self.tail_low()]
        return g

    # --- gating + emission (sim-first) ---
    def emit(self, gesture: Gesture):
        verdict, reason = self.gate_check(_gesture_skill(gesture.name))
        allowed = verdict == "grant"
        self.sink.render(gesture, allowed, reason)
        self.emitted.append((gesture.name, allowed))
        return allowed


def _gesture_skill(name):
    # gestures are gentle head/tail skills; KIRA treats them as "look_at_me"-class
    return "look_at_me"


class ConsoleSink:
    """Prints the gesture timeline (headless demo + logs)."""
    def render(self, g: Gesture, allowed: bool, reason: str):
        tgt = " ".join(f"{k}={v:+.2f}" for k, v in g.targets.items())
        osc = " ~" if g.oscillate else ""
        mark = "" if allowed else f"  [KIRA denied: {reason}]"
        extra = f"  ({g.note})" if g.note else ""
        print(f"    ~gesture {g.name:<9} {tgt}{osc}{mark}{extra}")


class CollectSink:
    """Collects the gesture timeline (+ reply markers) for the laptop viewer."""
    def __init__(self):
        import time
        self._t0 = time.time()
        self.events = []
    def render(self, g: Gesture, allowed: bool, reason: str):
        import time
        self.events.append({"t": time.time() - self._t0, "kind": "gesture",
                            "name": g.name, "targets": g.targets, "allowed": allowed})
    def mark(self, user, reply):
        import time
        self.events.append({"t": time.time() - self._t0, "kind": "say",
                            "user": user, "reply": reply})


def write_view_html(path, events):
    """A self-contained page (open directly) that replays BLUR's gestures on a
    little head+tail so Arjun can watch her react -- the laptop stand-in for
    the twin's neck/jaw/tail until the robot exists."""
    import json as _j
    html = """<!doctype html><meta charset=utf-8><title>BLUR reacts</title>
<style>body{font:14px system-ui;background:#0d0f12;color:#d7dce1;text-align:center;padding:16px}
#reply{min-height:2.4em;color:#8fb7ff;margin:8px auto;max-width:560px}
button{background:#1d2630;color:#cfe0ff;border:1px solid #2c3a49;border-radius:8px;padding:8px 14px}</style>
<h3>BLUR reacts (gesture replay)</h3>
<svg id=s width=360 height=320 viewBox="0 0 360 320"></svg>
<div id=reply>&nbsp;</div><button onclick=play()>replay</button>
<script>
const EV = __EVENTS__;
const S = document.getElementById('s'), R = document.getElementById('reply');
function head(pan,tilt,lift,jaw,tail){
  const cx=180, cy=150-lift*120, rot=pan*40, tl=tilt*30;
  return `<g transform="translate(${cx} ${cy}) rotate(${rot})">
    <ellipse rx=70 ry=${62+tl} fill=#1b2530 stroke=#39506b/>
    <polygon points="-70,-50 -40,-95 -20,-48" fill=#1b2530 stroke=#39506b/>
    <polygon points="70,-50 40,-95 20,-48" fill=#1b2530 stroke=#39506b/>
    <circle cx=-28 cy=-8 r=9 fill=#8fb7ff/><circle cx=28 cy=-8 r=9 fill=#8fb7ff/>
    <rect x=-18 y=${28} width=36 height=${4+jaw*34} rx=4 fill=#0d0f12 stroke=#39506b/>
  </g>
  <line x1=${cx+60} y1=${cy+70} x2=${cx+120+tail*120} y2=${cy+40} stroke=#39506b stroke-width=6/>`;
}
let st={pan:0,tilt:0,lift:0,jaw:0,tail:0};
function draw(){S.innerHTML=head(st.pan,st.tilt,st.lift,st.jaw,st.tail);}
function apply(t){const T=t.targets||{};
  if('neck_pan'in T)st.pan=T.neck_pan; if('neck_tilt'in T)st.tilt=T.neck_tilt;
  if('neck_lift'in T)st.lift=T.neck_lift; st.jaw=T.jaw||0;
  if('tail_winch_yaw'in T)st.tail=T.tail_winch_yaw; draw();}
function play(){st={pan:0,tilt:0,lift:0,jaw:0,tail:0};draw();let i=0;
  const t0=performance.now();
  (function step(){ if(i>=EV.length)return;
    const e=EV[i]; if((performance.now()-t0)/1000 >= e.t*0.5){
      if(e.kind=='gesture'){apply(e); setTimeout(()=>{st.jaw=0;if(e.name=='nod')st.tilt=0;draw();},300);}
      else R.textContent='BLUR: '+(e.reply||''); i++; }
    requestAnimationFrame(step); })();}
draw(); play();
</script>"""
    html = html.replace("__EVENTS__", _j.dumps(events))
    open(path, "w").write(html)


class TwinSink:
    """Drives the MuJoCo twin's neck/jaw/tail over the CAN protocol. Holds the
    other joints at the stand pose; only overrides the gesture actuators. Used
    when a twin socket is connected (sim-first); optional."""
    def __init__(self, send_setpoints):
        self.send = send_setpoints      # callable(dict actuator->angle)

    def render(self, g: Gesture, allowed: bool, reason: str):
        if allowed:
            self.send(g.targets)
