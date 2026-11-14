"""Gate E5, reduced: demonstrations needed against library size, on ONE task.

The pre-registration asks for four new tasks. This runs the curve on the
mug task only, and says so. For each library size (primitives added in
the fixed order below) it asks: can a recipe be learned from one
demonstration? If the library lacks a primitive the task needs, it
cannot, and the fallback is behaviour cloning of the whole task from N
demonstrations, N in {1, 2, 5, 10, 20, 50}. (The pre-registration says
only the missing segment is cloned; here the whole task is. That is a
weaker fallback and is the second deviation.)

    e5_gate.py demos FIRST LAST     teleoperated demonstrations on instances 2000+ (kept if they succeed)
    e5_gate.py train N              clone from the first N kept demonstrations
    e5_gate.py eval N FIRST LAST    run that clone on test seeds [FIRST, LAST)
    e5_gate.py report
"""
import glob, json, os, subprocess, sys, time
import torch
import arm_sim as A
import bc
import mug_task as M
import e4_gate as E4

OUT = os.path.join(E4.OUT, '..', 'e5')
ORDER = ['grasp', 'place', 'wipe', 'push', 'turn', 'pour']
NS = [1, 2, 5, 10, 20, 50]
RESULTS = os.path.join(OUT, 'results.json')


def load():
    return json.load(open(RESULTS)) if os.path.exists(RESULTS) else dict(clone={}, library={}, demos={})


def kept():
    return sorted(glob.glob(os.path.join(OUT, 'demo_*.log')))


def main():
    os.makedirs(OUT, exist_ok=True)
    r, cmd, t0 = load(), sys.argv[1], time.time()
    if cmd == 'demos':
        for i in range(int(sys.argv[2]), int(sys.argv[3])):
            path = os.path.join(OUT, f'demo_{i:04d}.log')
            try:
                v = M.demonstrate(path, M.sample(2000 + i))
            except AssertionError:
                v = dict(ok=False)
            r['demos'][str(i)] = bool(v['ok'])
            if not v['ok']:
                os.path.exists(path) and os.unlink(path)
            print(i, 'kept' if v['ok'] else 'discarded (the demonstration itself failed)')
        json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
    elif cmd == 'train':
        n = int(sys.argv[2])
        paths = kept()[:n]
        assert len(paths) == n, f'only {len(paths)} demonstrations kept'
        net, loss, samples = bc.train(paths, epochs=1500)
        torch.save(net, os.path.join(OUT, f'clone_{n}.pt'))
        r['clone'].setdefault(str(n), {}).update(loss=loss, samples=samples, trials={})
        json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
        print(f'clone from {n} demonstrations: {samples} samples, loss {loss:.5f}')
    elif cmd == 'eval':
        n = int(sys.argv[2])
        net = torch.load(os.path.join(OUT, f'clone_{n}.pt'), weights_only=False)
        secs = json.load(open(E4.RESULTS))['meta']['demo_seconds']
        for seed in range(int(sys.argv[3]), int(sys.argv[4])):
            v = bc.rollout(net, M.sample(seed), secs)
            r['clone'][str(n)]['trials'][str(seed)] = dict(ok=bool(v['ok']), covered=float(v['covered']))
            json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
        t = r['clone'][str(n)]['trials']
        print(f'clone from {n}: {sum(v["ok"] for v in t.values())}/{len(t)}')
    elif cmd == 'library':
        for size in range(len(ORDER) + 1):
            without = ORDER[size:]
            ok, msg = M.learn(E4.DEMO, os.path.join(OUT, f'recipe_{size}.txt'), without=without)
            r['library'][str(size)] = dict(have=ORDER[:size], learned=bool(ok), why=msg.splitlines()[0][:120])
            print(size, ORDER[:size], 'recipe learned' if ok else msg.splitlines()[0])
        json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
    if cmd == 'report':
        e4 = json.load(open(E4.RESULTS))['recipe']
        rate = 100 * sum(v['ok'] for v in e4.values()) / len(e4)
        best = {}
        print('behaviour cloning of the whole task, by number of demonstrations:')
        for n in NS:
            t = r['clone'].get(str(n), {}).get('trials', {})
            if t:
                best[n] = 100 * sum(v['ok'] for v in t.values()) / len(t)
                cov = sum(v['covered'] for v in t.values()) / len(t)
                print('  %2d demonstrations: %3.0f %% of %d trials, mean covered %.2f' % (n, best[n], len(t), cov))
        need_bc = next((n for n in NS if best.get(n, 0) >= 80), None)
        print('\nlibrary size -> demonstrations needed to reach 80 % on the mug task')
        for size in range(len(ORDER) + 1):
            row = r['library'].get(str(size))
            if not row:
                continue
            if row['learned']:
                need = '1   (recipe: %.0f %% on 100 novel mugs)' % rate
            else:
                need = ('%d (cloning)' % need_bc) if need_bc else 'not reached with up to %d (cloning; best %.0f %%)' % (
                    max(best) if best else 0, max(best.values()) if best else 0)
            print('  %d  %-42s %s' % (size, ','.join(row['have']) or '(none)', need))
    print('wall %.1f s' % (time.time() - t0))


if __name__ == '__main__':
    main()
