"""Gate E4: one demonstration, novel mugs.

    e4_gate.py setup                     make the demonstration, learn the recipe, train the clone
    e4_gate.py recipe FIRST LAST         run the recipe on test seeds [FIRST, LAST)
    e4_gate.py clone  FIRST LAST         run the behaviour-cloning baseline on the same seeds
    e4_gate.py report                    the table

Each invocation is one process and is sized to finish in under a minute
(about 40 instances). Results accumulate in motor/runs/e4/results.json.
Test seeds are 0..99. Seeds 1000 and up were used while the primitives
were being written and are never test seeds.
"""
import hashlib, json, os, sys, time
import torch
import bc
import mug_task as M

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'runs', 'e4')
DEMO, RECIPE, CLONE, RESULTS = (os.path.join(OUT, n) for n in ('demo.log', 'recipe.txt', 'clone.pt', 'results.json'))


def load():
    return json.load(open(RESULTS)) if os.path.exists(RESULTS) else dict(recipe={}, clone={}, meta={})


def save(r):
    json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)


def clean(v):
    return {k: (float(x) if hasattr(x, 'item') and not isinstance(x, bool) else x) for k, x in v.items()}


def main():
    os.makedirs(OUT, exist_ok=True)
    cmd = sys.argv[1]
    r = load()
    t0 = time.time()
    if cmd == 'setup':
        v = M.demonstrate(DEMO)
        ok, msg = M.learn(DEMO, RECIPE)
        assert ok, msg
        net, loss, n = bc.train([DEMO])
        torch.save(net, CLONE)
        demo = bc.read_demo(DEMO)
        r['meta'] = dict(demo_verdict=clean(v), demo_samples=len(demo),
                         demo_seconds=(demo[-1][0]['t'] - demo[0][0]['t']) / 1000,
                         demo_sha256=hashlib.sha256(open(DEMO, 'rb').read()).hexdigest(),
                         recipe=open(RECIPE).read(), learner=msg, clone_loss=loss, clone_samples=n,
                         demo_instance=M.DEMO)
        save(r)
        print('demonstration:', clean(v))
        print(msg)
        print(open(RECIPE).read())
        print('clone trained on %d samples, loss %.5f' % (n, loss))
    elif cmd in ('recipe', 'clone'):
        first, last = int(sys.argv[2]), int(sys.argv[3])
        net = torch.load(CLONE, weights_only=False) if cmd == 'clone' else None
        for seed in range(first, last):
            c = M.sample(seed)
            assert c != M.DEMO
            if cmd == 'recipe':
                v, sim = M.run_recipe(RECIPE, c)
                v['ended'] = [u.split()[3] for u in sim.ups if ' ended ' in u and u.split()[1].isdigit()]
                v['gate_calls'] = int(sim.stat().get('gate_calls', 0))
                sim.close()
            else:
                v = bc.rollout(net, c, r['meta']['demo_seconds'])
            r[cmd][str(seed)] = clean(v)
            print(seed, 'ok ' if v['ok'] else 'BAD', 'covered %.2f moved %.3f back %.3f' % (
                v['covered'], v['mug_moved'], v['brush_from_holder']), v.get('done', ''), v.get('ended', ''))
            save(r)
    if cmd == 'report' or cmd in ('recipe', 'clone'):
        for name in ('recipe', 'clone'):
            rows = r[name]
            if rows:
                n, ok = len(rows), sum(1 for v in rows.values() if v['ok'])
                cov = sum(v['covered'] for v in rows.values()) / n
                print('%-7s %d/%d succeed (%.0f %%), mean covered %.2f' % (name, ok, n, 100 * ok / n, cov))
    print('wall %.1f s' % (time.time() - t0))


if __name__ == '__main__':
    main()
