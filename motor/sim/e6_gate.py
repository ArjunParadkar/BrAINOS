"""Gate E6: fluidity. The same task, body and recipe, two ways of moving.

    fluid     overlapping strokes, each with its own stiffness, the next
              primitive asked for ahead of time from predicted positions
    stop-go   the baseline: fixed full stiffness, a full stop and a 150 ms
              pause after every stroke, and between primitives a wait for
              a fresh look at the world

    e6_gate.py run FIRST LAST     both styles on test seeds [FIRST, LAST)
    e6_gate.py report

Metrics are computed from the tool tip's position, sampled every
millisecond, between the first and the last moment the tip moves faster
than 5 % of its peak speed.
"""
import json, os, sys, time
import numpy as np
import mug_task as M
import e4_gate as E4

RESULTS = os.path.join(E4.OUT, '..', 'e6', 'results.json')


def metrics(sim):
    log = np.array([r[:4] for r in sim.log], dtype=float)
    t, p = log[:, 0] * 1e-6, log[:, 1:4]
    # smooth over 20 ms before differentiating three times: the raw
    # millisecond positions carry contact chatter that would swamp it
    k = np.ones(20) / 20
    p = np.stack([np.convolve(p[:, i], k, mode='valid') for i in range(3)], axis=1)
    dt = 0.001
    v = np.gradient(p, dt, axis=0)
    speed = np.linalg.norm(v, axis=1)
    moving = np.nonzero(speed > 0.05 * speed.max())[0]
    a, b = moving[0], moving[-1]
    j = np.gradient(np.gradient(v, dt, axis=0), dt, axis=0)[a:b]
    jerk = float((np.linalg.norm(j, axis=1) ** 2).sum() * dt)
    slow = speed[a:b] < 0.05 * speed.max()
    pauses, run = 0, 0
    for s in slow:
        run = run + 1 if s else 0
        if run == 100:
            pauses += 1
    return dict(jerk=jerk, pauses=pauses, seconds=float((b - a) * dt), peak_speed=float(speed.max()))


def main():
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    r = json.load(open(RESULTS)) if os.path.exists(RESULTS) else {'fluid': {}, 'stop-go': {}}
    if sys.argv[1] == 'run':
        t0 = time.time()
        for seed in range(int(sys.argv[2]), int(sys.argv[3])):
            for style in ('fluid', 'stop-go'):
                v, sim = M.run_recipe(E4.RECIPE, M.sample(seed), style=style, record=True, max_ms=90_000)
                m = metrics(sim)
                m.update(ok=v['ok'], peak_tool_n=float(v['peak_tool_n']), done=v['done'], covered=v['covered'], mug_moved=float(v['mug_moved']), back=float(v['brush_from_holder']))
                sim.close()
                r[style][str(seed)] = m
                json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
                print(seed, style, 'ok' if m['ok'] else 'BAD', 'jerk %.3g pauses %d peak %.1f N  %.1f s' % (
                    m['jerk'], m['pauses'], m['peak_tool_n'], m['seconds']), m['done'])
        print('wall %.1f s' % (time.time() - t0))
    f, s = r['fluid'], r['stop-go']
    both = sorted(set(f) & set(s), key=int)
    if not both:
        return
    med = lambda rows, k: float(np.median([rows[i][k] for i in both]))
    ok = lambda rows: sum(1 for i in both if rows[i]['ok'])
    print('\n%d instances run both ways' % len(both))
    print('%-28s %12s %12s %8s   %s' % ('', 'fluid', 'stop-go', 'ratio', 'threshold'))
    rows = [('integrated squared jerk', 'jerk', '<= 0.50'), ('pauses', 'pauses', '<= 0.50'),
            ('peak contact force, N', 'peak_tool_n', '<= 0.70'), ('seconds', 'seconds', '(reported)')]
    for label, key, thr in rows:
        a, b = med(f, key), med(s, key)
        print('%-28s %12.4g %12.4g %8.2f   %s' % (label + ' (median)', a, b, a / b if b else float('nan'), thr))
    print('%-28s %9d/%d %9d/%d %8s   %s' % ('task success', ok(f), len(both), ok(s), len(both), '',
                                             'fluid no more than 5 points below'))


if __name__ == '__main__':
    main()
