"""Quantise a trained actor to the int8 blob `brainos_motor::policy::Int8Mlp` runs.

    $MOTOR_PY motor/gait/export_int8.py RUN/first_stable.pt --out RUN/policy.bmoti8 \
        --golden-dir RUN/golden --record 1000

Scheme (all of it is forced by the blob format: one input scale per
input, one weight scale per layer, one mult/shift per layer, int32 bias):

* inputs: in_scale = 127 / OBS_RANGE, so the integer input is
  127 * clip(obs / OBS_RANGE, -1, 1), the same clip the float network
  was trained with. Layer 1's input scale is therefore exactly 127.
* weights: symmetric per layer, s_w = 127 / max|W|.
* bias: rint(b * s_w * s_in), int32.
* hidden activations: s_out = 127 / (the --pct percentile of that
  layer's float activations on calibration observations); anything
  above saturates at 127, as the runner clamps.
* last layer: s_out = 127 / out_scale with out_scale = 1, since actions
  live in [-1, 1] and the runner clamps to +-127.
* requantisation: M = s_out / (s_w * s_in) ~ mult / 2^shift, with mult
  in [2^14, 2^15) where the shift range allows; checked so that
  |acc * mult| stays far inside int64 for any input.

Calibration observations come from the float policy driving the model
(nominal and randomised). The 1,000 recorded observations for the gate
come from the *int8* policy driving the nominal model on fresh seeds;
they and the reference's integer outputs are written as a golden set
for `os/motor/tests/policy_golden.rs` (BRAINOS_POLICY_GOLDEN_DIR).
"""
import argparse, json, math, os, struct, sys, time
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import int8_ref
from env import OBS_RANGE, HexEnv, N_OBS, model_sha256

INT32 = (-(2 ** 31), 2 ** 31 - 1)


def requant(M, max_acc):
    if not (M > 0 and math.isfinite(M)):
        raise ValueError(f'bad requantisation factor {M}')
    shift = int(math.floor(math.log2(2 ** 15 / M)))
    shift = min(max(shift, 1), 40)
    mult = int(round(M * 2 ** shift))
    while mult >= 2 ** 15 and shift > 1:          # rounding pushed it up
        shift -= 1
        mult = int(round(M * 2 ** shift))
    if not (0 < mult < 2 ** 31):
        raise ValueError(f'mult {mult} out of int32 (M={M})')
    if max_acc * mult + (1 << (shift - 1)) >= 2 ** 62:
        raise ValueError('requantisation could overflow int64')
    return mult, shift


def quantise(layers, calib_obs, out_scale=1.0, pct=99.99):
    """layers: [(W float32 rows x cols, b float32), ...] of a ReLU MLP whose
    input is clip(obs / OBS_RANGE, -1, 1). Returns (blob bytes, report)."""
    n_in = layers[0][0].shape[1]
    assert n_in == N_OBS
    in_scale = (np.float32(127.0) / OBS_RANGE.astype(np.float32)).astype(np.float32)
    x = np.clip(np.asarray(calib_obs, np.float32) / OBS_RANGE, -1.0, 1.0)
    s_in = 127.0
    max_a = 127
    parts = [int8_ref.MAGIC, struct.pack('<HH', n_in, len(layers)), in_scale.astype('<f4').tobytes()]
    report = []
    for k, (W, b) in enumerate(layers):
        last = k == len(layers) - 1
        rows, cols = W.shape
        wmax = float(np.abs(W).max())
        s_w = 127.0 / wmax if wmax > 0 else 1.0
        Wq = np.clip(np.rint(W * s_w), -127, 127).astype(np.int8)
        bq = np.clip(np.rint(b.astype(np.float64) * s_w * s_in), *INT32).astype(np.int64)
        y = x @ W.T + b
        if last:
            s_out = 127.0 / out_scale
        else:
            y = np.maximum(y, 0.0)
            top = float(np.percentile(y, pct)) if y.size else 0.0
            s_out = 127.0 / top if top > 0 else 1.0
        max_acc = int(np.abs(Wq.astype(np.int64)).sum(1).max()) * max_a + int(np.abs(bq).max())
        mult, shift = requant(s_out / (s_w * s_in), max_acc)
        parts += [struct.pack('<HHiB', rows, cols, mult, shift), Wq.tobytes(),
                  bq.astype('<i4').tobytes()]
        report.append(dict(layer=k, rows=rows, cols=cols, s_w=s_w, s_in=s_in, s_out=s_out,
                           mult=mult, shift=shift, M=s_out / (s_w * s_in),
                           M_err=mult / 2 ** shift / (s_out / (s_w * s_in)) - 1,
                           sat=float((y * s_out > 127.5).mean()) if not last else None))
        x = y if not last else x
        s_in = s_out
    parts.append(struct.pack('<f', out_scale))
    return b''.join(parts), report


def rollout_obs(fn, n, seed, dr, seconds=10.0):
    """Observations met while `fn` drives the model, n of them."""
    out, k = [], 0
    while len(out) < n:
        e = HexEnv(dr=dr, max_steps=10 ** 9)
        rng = np.random.default_rng([seed, k])
        o = e.reset(rng)
        for _ in range(int(seconds * 50)):
            out.append(o.copy())
            o, _, term, _, _ = e.step(fn(o))
            if term or len(out) >= n:
                break
        k += 1
    return np.stack(out[:n]).astype(np.float32)


def main():
    import torch
    from ppo import Agent, actor_numpy, float_policy
    ap = argparse.ArgumentParser()
    ap.add_argument('ckpt')
    ap.add_argument('--out', required=True)
    ap.add_argument('--calib', type=int, default=4000, help='calibration observations')
    ap.add_argument('--pct', type=float, default=99.99, help='hidden activation percentile mapped to 127')
    ap.add_argument('--record', type=int, default=1000, help='observations recorded for the bit-exact check')
    ap.add_argument('--golden-dir', default=None)
    ap.add_argument('--seed', type=int, default=123)
    a = ap.parse_args()
    t0 = time.time()
    torch.set_num_threads(1)
    ck = torch.load(a.ckpt, map_location='cpu', weights_only=False)
    agent = Agent(hidden=tuple(ck['config']['hidden']))
    agent.load_state_dict(ck['agent'])
    layers = actor_numpy(agent.actor)
    fp = float_policy(agent.actor)
    calib = np.concatenate([rollout_obs(fp, a.calib // 2, a.seed, dr=False),
                            rollout_obs(fp, a.calib - a.calib // 2, a.seed + 1, dr=True)])
    blob, rep = quantise(layers, calib, pct=a.pct)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, 'wb') as f:
        f.write(blob)
    p = int8_ref.parse(blob)
    qf = lambda o: int8_ref.act(p, o).astype(np.float64)
    rec = rollout_obs(qf, a.record, a.seed + 2, dr=False)
    yf = np.stack([fp(o) for o in rec])
    yq = int8_ref.act(p, rec)
    summary = dict(ckpt=os.path.abspath(a.ckpt), blob=os.path.abspath(a.out), bytes=len(blob),
                   model_sha256=model_sha256(), pct=a.pct, layers=rep,
                   float_vs_int8_on_recorded=dict(max_abs=float(np.abs(yf - yq).max()),
                                                  mean_abs=float(np.abs(yf - yq).mean())),
                   seconds=time.time() - t0)
    if a.golden_dir:
        int8_ref.write_golden(a.golden_dir, blob, rec, dict(
            what='trained policy, observations recorded while the int8 policy drove the nominal model',
            ckpt=summary['ckpt'], seed=a.seed + 2))
        summary['golden_dir'] = os.path.abspath(a.golden_dir)
    with open(a.out + '.json', 'w') as f:
        json.dump(summary, f, indent=1)
    print(json.dumps({k: v for k, v in summary.items() if k != 'layers'}, indent=1))
    for r in rep:
        print('layer %d %dx%d mult %d shift %d (M rel err %.1e) hidden sat %s' % (
            r['layer'], r['rows'], r['cols'], r['mult'], r['shift'], r['M_err'], r['sat']))


if __name__ == '__main__':
    main()
