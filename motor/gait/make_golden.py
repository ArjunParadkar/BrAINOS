"""Regenerate os/motor/tests/golden/: a fixed int8 blob, 1,000 recorded
observations and the integer actions the Python reference computes, for
`os/motor/tests/policy_golden.rs`.

    $MOTOR_PY motor/gait/make_golden.py          # a few seconds

The network is NOT trained: weights are drawn from a seeded normal
distribution (He scale), same shape as the gait actor (36-64-64-10),
so the test checks the runner's arithmetic, not a gait. The 1,000
observations are recorded from the generated hexapod driven by seeded
random actions, so they have the ranges a real observation has
(including values past the clip). Hidden activations are calibrated at
the 99th percentile and the output layer is left unscaled, so some
hidden units saturate at 127 and some outputs clamp at +-127; the
counts are written to meta.json.

A second set, edge/, holds 32 crafted rows (each a recorded row with a
few inputs replaced): products x * in_scale that land exactly on k + 0.5
for even and odd k of both signs (ties to even), NaN, +-inf, +-1e30,
-0.0 and a subnormal. The same blob is used.
"""
import json, os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import int8_ref
from env import HexEnv, N_ACT, N_OBS
from export_int8 import quantise

OUT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    '..', '..', 'os', 'motor', 'tests', 'golden'))
SEED = 20261112


def record(n, seed):
    rng = np.random.default_rng(seed)
    rows, k = [], 0
    while len(rows) < n:
        e = HexEnv(dr=True, max_steps=10 ** 9)
        o = e.reset(np.random.default_rng([seed, k]))
        for _ in range(250):
            rows.append(o.copy())
            o, _, term, _, _ = e.step(rng.normal(0.0, 0.7, N_ACT))
            if term or len(rows) >= n:
                break
        k += 1
    return np.stack(rows).astype(np.float32)


def tie_value(scale, k):
    """A float32 x with float32(x * scale) == k + 0.5 exactly, or None."""
    target = np.float32(k + 0.5)
    x = np.float32(target / scale)
    for _ in range(64):
        p = np.float32(x * scale)
        if p == target:
            return x
        x = np.nextafter(x, np.float32(np.inf) if p < target else np.float32(-np.inf), dtype=np.float32)
    return None


def main():
    rng = np.random.default_rng(SEED)
    sizes = [N_OBS, 64, 64, N_ACT]
    layers = [(rng.normal(0, np.sqrt(2.0 / a), (b, a)).astype(np.float32),
               rng.normal(0, 0.1, b).astype(np.float32)) for a, b in zip(sizes[:-1], sizes[1:])]
    obs = record(1000, SEED)
    blob, rep = quantise(layers, obs, out_scale=1.0, pct=99.0)
    p = int8_ref.parse(blob)

    # edge rows
    edge = obs[:32].copy()
    ties = []
    for k in (0, 1, 2, 3, 10, 11, 62, 63, -1, -2, -3, -4, -11, -12, 125, -126):
        for i in range(N_OBS):
            x = tie_value(p['in_scale'][i], k)
            if x is not None:
                ties.append((i, x))
                break
    for r, (i, x) in enumerate(ties):          # one tie per row, rows 0..15
        edge[r % 16, i] = x
    specials = [np.nan, np.inf, -np.inf, 1e30, -1e30, -0.0, np.float32(1e-40)]
    for j, v in enumerate(specials):
        for row in range(16 + 2 * j, 18 + 2 * j):
            edge[row, (row * 7 + j) % N_OBS] = np.float32(v)
    edge[30, :] = np.nan
    edge[31, :] = np.float32(1e30)

    q = int8_ref.write_golden(OUT, blob, obs, dict(
        what='random seeded network (not trained); observations recorded from the generated '
             'hexapod under seeded random actions', seed=SEED, layers=rep,
        clamped_outputs=0, saturated_hidden=0, regenerate='motor/gait/make_golden.py'))
    int8_ref.write_golden(os.path.join(OUT, 'edge'), blob, edge, dict(
        what='crafted rows: ties to even, NaN, inf, huge, -0.0, subnormal', seed=SEED,
        ties=[(int(i), float(x)) for i, x in ties], regenerate='motor/gait/make_golden.py'))

    # coverage, so a test that passes is known to have exercised the clamps
    a = int8_ref.quantise_input(p, obs)
    hid_sat = 0
    for l in p['layers'][:-1]:
        y = (a @ l['w'].T + l['b']) * l['mult'] + (1 << (l['shift'] - 1)) >> l['shift']
        hid_sat += int((y > 127).sum())
        a = np.clip(y, 0, 127)
    mp = os.path.join(OUT, 'meta.json')
    meta = json.load(open(mp))
    meta.update(clamped_outputs=int((np.abs(q) == 127).sum()), saturated_hidden=hid_sat,
                negative_outputs=int((q < 0).sum()), zero_outputs=int((q == 0).sum()),
                input_clamped=int((np.abs(int8_ref.quantise_input(p, obs)) == 127).sum()))
    json.dump(meta, open(mp, 'w'), indent=1)
    print(f'wrote {OUT}: blob {len(blob)} bytes, {len(obs)} obs, {len(ties)} tie inputs, '
          f'{meta["clamped_outputs"]} clamped outputs, {hid_sat} saturated hidden values, '
          f'{meta["input_clamped"]} clamped inputs')


if __name__ == '__main__':
    main()
