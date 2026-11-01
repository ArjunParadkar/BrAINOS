#!/usr/bin/env python3
"""
schema_ph.py -- B3: persistent homology of each discovered module's latent,
judged by the rule fixed in PREREGISTRATION.md, with its controls.

  python schema_ph.py runs/m0 [--control-run runs/m0_shuffled]
Writes runs/m0/ph.json and runs/m0/ph_<module>.png.
"""
import os, sys, json, math, argparse, time
import numpy as np
from ripser import ripser
from sklearn.neighbors import KNeighborsRegressor
from sklearn.model_selection import cross_val_score, KFold
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

N_PTS = 400       # amended 2026-10-22 before any latent was analysed: 1500 points OOM'd (15.5 GB)
N_PTS_H3 = 400
REL_DIAM = 0.15
REL_MED = 5.0


def fps(X, n, seed=0):
    rng = np.random.default_rng(seed)
    if len(X) <= n:
        return X
    idx = [int(rng.integers(len(X)))]
    d = np.linalg.norm(X - X[idx[0]], axis=1)
    for _ in range(n - 1):
        i = int(np.argmax(d)); idx.append(i)
        d = np.minimum(d, np.linalg.norm(X - X[i], axis=1))
    return X[idx]


def pca_dim(X, frac=0.95):
    X = X - X.mean(0)
    s = np.linalg.svd(X, compute_uv=False) ** 2
    return int(np.searchsorted(np.cumsum(s) / s.sum(), frac) + 1)


def betti(dgms, diam):
    """the pre-registered rule: persistence >= 0.15 x diameter AND >= 5 x the degree's median"""
    out = []
    for k, D in enumerate(dgms):
        fin = D[np.isfinite(D[:, 1])]
        pers = fin[:, 1] - fin[:, 0] if len(fin) else np.zeros(0)
        # amendment 2: each bar against the median of the OTHER bars of its
        # degree (a lone feature would otherwise be compared with itself)
        n = 0
        for i, q in enumerate(pers):
            others = np.delete(pers, i)
            med = float(np.median(others)) if len(others) else 0.0
            n += int(q >= REL_DIAM * diam and q >= REL_MED * med)
        if k == 0:
            n += int(np.sum(~np.isfinite(D[:, 1])))
        out.append(n)
    return out


def ph(X, maxdim, tag, png=None):
    X = (X - X.mean(0)) / (X.std(0) + 1e-9)
    n = N_PTS_H3 if maxdim >= 3 else N_PTS
    P = fps(X, n)
    diam = float(np.max(np.linalg.norm(P[:, None] - P[None], axis=-1)))
    res = {}
    for p in (2, 3):
        t = time.time()
        dg = ripser(P, maxdim=maxdim, coeff=p)['dgms']
        res[f'Z{p}'] = dict(betti=betti(dg, diam), seconds=round(time.time() - t, 1),
                            top_persistence=[sorted([float(b - a) for a, b in D if np.isfinite(b)], reverse=True)[:4] for D in dg])
        if png and p == 2:
            fig, ax = plt.subplots(1, 1, figsize=(4, 4))
            for k, D in enumerate(dg):
                fin = D[np.isfinite(D[:, 1])]
                ax.scatter(fin[:, 0], fin[:, 1], s=8, label=f'H{k}')
            m = max(1e-9, max((D[np.isfinite(D[:, 1])][:, 1].max() if np.isfinite(D[:, 1]).any() else 0) for D in dg))
            ax.plot([0, m], [0, m], 'k--', lw=0.5); ax.set_title(tag, fontsize=8); ax.legend(fontsize=7)
            fig.tight_layout(); fig.savefig(png, dpi=110); plt.close(fig)
    return dict(points=len(P), diameter=round(diam, 3), maxdim=maxdim, **res)


def quat_to_R(q):
    w, x, y, z = q.T
    return np.stack([1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y),
                     2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x),
                     2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)], 1)


def targets(L):
    R = quat_to_R(L['gt_chest_quat'].astype(np.float64)).reshape(-1, 3, 3)
    g = np.einsum('nji,j->ni', R, np.array([0, 0, -1.0]))          # gravity in the chest frame
    H = L['gt_head_xmat'].reshape(-1, 3, 3)
    yaw = np.arctan2(H[:, 1, 0], H[:, 0, 0])
    names = [str(c)[4:] for c in L['channels'] if str(c).startswith('enc:')]
    T = {'gravity_dir(3)': g, 'head_world_yaw(cos,sin)': np.stack([np.cos(yaw), np.sin(yaw)], 1),
         'chest_attitude(R,9)': R.reshape(-1, 9)}
    q = L['gt_q']
    allnames = ['FR_abd', 'FR_pitch', 'FR_knee', 'FR_dist', 'FL_abd', 'FL_pitch', 'FL_knee', 'FL_dist', 'RR_abd', 'RR_pitch',
                'RR_knee', 'RR_dist', 'RL_abd', 'RL_pitch', 'RL_knee', 'RL_dist', 'spine_s1', 'spine_twist', 'spine_s2',
                'tail_winch_pitch', 'tail_winch_yaw', 'neck_lift', 'neck_pan', 'neck_tilt', 'jaw', 'gripper']
    for i, n in enumerate(allnames):
        T[f'q:{n}'] = q[:, i:i + 1]
    for leg in ('FR', 'FL', 'RR', 'RL'):
        T[f'leg:{leg}(4 joints)'] = q[:, [allnames.index(f'{leg}_{j}') for j in ('abd', 'pitch', 'knee', 'dist')]]
    return T, g, R.reshape(-1, 9)


def decode_r2(Z, Y, n=6000, seed=0):
    rng = np.random.default_rng(seed)
    i = rng.choice(len(Z), min(n, len(Z)), replace=False)
    Zs = (Z[i] - Z.mean(0)) / (Z.std(0) + 1e-9)
    s = cross_val_score(KNeighborsRegressor(10), Zs, Y[i], cv=KFold(5, shuffle=True, random_state=0), scoring='r2')
    return float(np.mean(s))


def main():
    p = argparse.ArgumentParser()
    p.add_argument('run'); p.add_argument('--control-run', default=None)
    p.add_argument('--skip-controls', action='store_true')
    a = p.parse_args()
    L = dict(np.load(os.path.join(a.run, 'latents.npz')))
    Z, A, ch = L['Z'], L['A'], [str(c) for c in L['channels']]
    hard = A.argmax(1)
    T, g, R9 = targets(L)
    out = dict(modules={}, controls={})
    if not a.skip_controls:
        print('controls ...', flush=True)
        out['controls']['gravity_dir (expect S2)'] = ph(g, 2, 'control: gravity', os.path.join(a.run, 'ph_control_gravity.png'))
        out['controls']['chest_attitude R (expect RP3)'] = ph(R9, 2, 'control: attitude', os.path.join(a.run, 'ph_control_attitude.png'))
        out['controls']['gaussian 6-D (expect point)'] = ph(np.random.default_rng(1).normal(size=(30000, 6)), 2, 'control: gaussian')
        if a.control_run:
            Lc = dict(np.load(os.path.join(a.control_run, 'latents.npz')))
            hc = Lc['A'].argmax(1)
            for k in sorted(set(hc.tolist())):
                out['controls'][f'shuffled-data module {k}'] = ph(Lc['Z'][:, k], 2, f'shuffled m{k}')
        print(json.dumps(out['controls'], indent=1), flush=True)
    for k in sorted(set(hard.tolist())):
        chans = [c for c, h in zip(ch, hard) if h == k]
        Zk = Z[:, k]
        dim = pca_dim(Zk)
        r2 = {name: decode_r2(Zk, Y) for name, Y in T.items()}
        top = sorted(r2.items(), key=lambda x: -x[1])[:6]
        md = 2       # amended: H1/H2 over Z/2 vs Z/3 already separate S2 from RP3; H3 is not computed
        print(f'module {k}: {len(chans)} channels, pca95 dim {dim}, top decodes {[(n, round(v, 2)) for n, v in top[:3]]}', flush=True)
        res = ph(Zk, md, f'module {k}', os.path.join(a.run, f'ph_m{k}.png'))
        out['modules'][str(k)] = dict(channels=chans, pca95_dim=dim, decode_r2=dict(top), ph=res)
        print('   ', json.dumps(res), flush=True)
        json.dump(out, open(os.path.join(a.run, 'ph.json'), 'w'), indent=1)
    json.dump(out, open(os.path.join(a.run, 'ph.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
