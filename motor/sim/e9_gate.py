"""Gate E9, the standing half: posture held under a standing grant
survives the loss of cognition.

The arm grasps the brush (cognition asks for it), then the cognition
process is sent SIGKILL. For the next ten seconds the arm must go on
holding its posture and the brush, under the standing grant alone, with
nothing asking the gate anything.

    e9_gate.py FIRST LAST
"""
import json, os, sys, time
import numpy as np
import mug_task as M
import e4_gate as E4


def trial(c):
    sim = M.scene(c)
    sim.run(200)
    sim.start_mind(E4.RECIPE, stop_go=True)      # asks for one primitive at a time
    ok = sim.run_until(lambda: sim.ended(0) is not None, 12_000)
    sim.kill_mind()
    gone = sim.mind.poll() is not None
    asked_before = int(sim.stat()['gate_calls'])
    at, drift, standing, held = sim.tool_pos().copy(), 0.0, 0, 0
    for _ in range(10_000):
        d = sim.step()
        drift = max(drift, float(np.linalg.norm(sim.tool_pos() - at)))
        standing += d['standing']
        held += sim.held == sim.body('brush')
    st = sim.stat()
    out = dict(grasped=bool(ok and sim.ended(0) == 'finished'), mind_gone=gone, standing_ticks=standing,
               held_ticks=int(held), drift_m=drift, gate_calls_after_kill=int(st['gate_calls']) - asked_before,
               refused=int(st['refused']), later_asks=[u for u in sim.ups if u.startswith('up 1')])
    sim.close()
    return out


if __name__ == '__main__':
    t0 = time.time()
    rows = []
    for seed in range(int(sys.argv[1]), int(sys.argv[2])):
        o = trial(M.sample(seed))
        rows.append(o)
        print(seed, o)
    n = len(rows)
    good = [o for o in rows if o['grasped'] and o['mind_gone'] and o['standing_ticks'] == 10_000
            and o['held_ticks'] == 10_000 and o['gate_calls_after_kill'] == 0 and o['drift_m'] < 0.005]
    print('\n%d/%d: cognition killed, posture and brush held for all 10,000 ticks, drift under 5 mm, gate not asked'
          % (len(good), n))
    print('worst drift %.2f mm' % (1000 * max(o['drift_m'] for o in rows)))
    os.makedirs(os.path.join(E4.OUT, '..', 'e9'), exist_ok=True)
    json.dump(rows, open(os.path.join(E4.OUT, '..', 'e9', 'results.json'), 'w'), indent=1)
    print('wall %.1f s' % (time.time() - t0))
