"""Gate E7: reflex arcs, with cognition alive and with cognition killed.

Three events, each provoked during the mug task on test instances:

  slip    the brush handle turns slippery while it is being carried
  touch   something pushes on the hand while it is moving through free air
  shove   a harder, longer push knocks the tool off its path

Latency is counted in 1 ms ticks from the first tick whose sensor frame
contains the event to the first tick whose command answers it. "Killed"
means the cognition process was sent SIGKILL (and reaped) 100 ms before
the event; the seat and the arm's loop are a different process.

    e7_gate.py run FIRST LAST
    e7_gate.py report
"""
import json, os, sys, time
import mujoco
import numpy as np
import arm_sim as A
import mug_task as M
import e4_gate as E4

RESULTS = os.path.join(E4.OUT, '..', 'e7', 'results.json')
G = mujoco.mjtObj.mjOBJ_GEOM


def reference_path(c):
    """The tool's path with nothing disturbing it."""
    v, sim = M.run_recipe(E4.RECIPE, c, record=True)
    path = np.array([r[1:4] for r in sim.log[::5]])
    sim.close()
    return path


def off_path(path, p):
    return float(np.min(np.linalg.norm(path - p, axis=1)))


def trial(c, event, kill, path=None):
    sim = M.scene(c, record=True)
    sim.run(200)
    sim.start_mind(E4.RECIPE)
    hand = sim.body('hand')
    handle = mujoco.mj_name2id(sim.m, G, 'brush_handle')
    out = dict(event=event, killed=kill, mind_gone=False)
    # when the event happens: a fixed time after the stroke it belongs to begins
    armed_at = None
    t_event = None
    felt_at = answered_at = None
    base_squeeze = None
    worst = 0.0
    back_at = None
    push_end = None
    for _ in range(40_000):
        d = sim.step()
        k = sim.t_us // 1000
        if armed_at is None:
            if event == 'slip':
                # carrying: the wipe's first stroke, brush in the hand and clear of the table
                ready = d['running'] and sim.held == sim.body('brush') and sim.pos('brush')[2] > 0.04
            else:
                # the approach to the brush, through free air, before any touch is expected
                ready = d['running'] and d['goal'] == 0 and sim.held < 0
            if ready:
                armed_at = k
                t_event = k + (250 if event == 'slip' else 300)
                base_squeeze = d['squeeze']
        elif t_event is not None:
            if kill and not out['mind_gone'] and k >= t_event - 100:
                sim.kill_mind()
                out['mind_gone'] = sim.mind.poll() is not None
            if k == t_event:
                if event == 'slip':
                    sim.m.geom_friction[handle, 0] = 0.02
                elif event == 'touch':
                    sim.push = (hand, np.array([0.0, 4.0, 0.0]), sim.t_us + 60_000)
                else:
                    sim.push = (hand, np.array([0.0, 9.0, 0.0]), sim.t_us + 150_000)
                    push_end = k + 150
            if k > t_event:
                if event == 'slip':
                    if felt_at is None and sim.last['slip'] > 0.004:
                        felt_at = k
                    if felt_at is not None and answered_at is None and d['squeeze'] > base_squeeze + 0.5:
                        answered_at = k
                elif event == 'touch':
                    if felt_at is None and sim.last['tool_n'] > 1.5:
                        felt_at = k
                    if felt_at is not None and answered_at is None and d['running'] and d['stiff'] <= 0.2:
                        answered_at = k
                else:
                    e = off_path(path, sim.tool_pos())
                    if k <= push_end + 50:
                        worst = max(worst, e)
                    if k > push_end and back_at is None and e <= 0.02:
                        back_at = k
                if k > t_event + 1500:
                    break
    if event == 'slip':
        out['held_after'] = bool(sim.held == sim.body('brush'))
        out['squeeze_after'] = sim.drive['squeeze']
    if event == 'shove':
        out.update(worst_off_path=worst, recover_ms=(back_at - push_end) if back_at else None,
                   pushed_off=worst > 0.02)
    else:
        # the frame that contains the event is sensed in the tick after the
        # one in which it was first true in the simulator
        out['latency_ms'] = (answered_at - felt_at) if (felt_at and answered_at) else None
        out['felt'] = felt_at is not None
    sim.close()
    return out


def main():
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    r = json.load(open(RESULTS)) if os.path.exists(RESULTS) else {}
    if sys.argv[1] == 'run':
        t0 = time.time()
        for seed in range(int(sys.argv[2]), int(sys.argv[3])):
            c = M.sample(seed)
            path = reference_path(c)
            for event in ('slip', 'touch', 'shove'):
                for kill in (False, True):
                    o = trial(c, event, kill, path)
                    r[f'{event}/{"killed" if kill else "alive"}/{seed}'] = o
                    json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
                    print(seed, event, 'killed' if kill else 'alive ', {k: v for k, v in o.items() if k not in ('event', 'killed')})
        print('wall %.1f s' % (time.time() - t0))
    thresholds = dict(slip=20, touch=10, shove=500)
    print('\n%-6s %-7s %3s  %-34s %s' % ('event', 'mind', 'n', 'latency ms: median / worst', 'within threshold'))
    for event in ('slip', 'touch', 'shove'):
        for mind in ('alive', 'killed'):
            rows = [v for k, v in r.items() if k.startswith(f'{event}/{mind}/')]
            if not rows:
                continue
            key = 'recover_ms' if event == 'shove' else 'latency_ms'
            vals = [v[key] for v in rows]
            got = [v for v in vals if v is not None]
            ok = sum(1 for v in got if v <= thresholds[event])
            extra = ''
            if event == 'slip':
                extra = '  still held: %d/%d' % (sum(v['held_after'] for v in rows), len(rows))
            if event == 'shove':
                extra = '  pushed >2 cm off: %d/%d, worst %.1f cm' % (
                    sum(v['pushed_off'] for v in rows), len(rows), 100 * max(v['worst_off_path'] for v in rows))
            if mind == 'killed':
                extra += '  SIGKILL confirmed: %d/%d' % (sum(v['mind_gone'] for v in rows), len(rows))
            print('%-6s %-7s %3d  %-34s %d/%d (<= %d ms)%s' % (
                event, mind, len(rows),
                ('%.0f / %.0f' % (np.median(got), max(got))) if got else 'never answered',
                ok, len(rows), thresholds[event], extra))


if __name__ == '__main__':
    main()
