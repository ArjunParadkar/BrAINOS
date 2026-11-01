#!/usr/bin/env python3
"""assign_check.py -- topology-blind quality of a schema model's assignment:
crispness, coherence (do a joint's cmd/enc/cur and the IMU land together),
and agreement between seeds (adjusted Rand index). Used to choose the model
BEFORE any persistent homology is looked at."""
import sys, numpy as np
from sklearn.metrics import adjusted_rand_score


def load(run):
    L = np.load(f'{run}/latents.npz'); A = L['A']; ch = [str(c) for c in L['channels']]
    return A, ch


def coherence(A, ch):
    h = A.argmax(1); idx = {c: i for i, c in enumerate(ch)}
    joints = sorted({c.split(':')[1] for c in ch if c.startswith('enc:')})
    same = []
    for j in joints:
        ks = [h[idx[f'{f}:{j}']] for f in ('cmd', 'enc', 'cur') if f'{f}:{j}' in idx]
        same.append(len(set(ks)) == 1)
    imu = [h[idx[c]] for c in ch if c.startswith('imu:a')]
    return float(np.mean(same)), len(set(imu))


if __name__ == '__main__':
    runs = sys.argv[1:]
    hs = []
    for r in runs:
        A, ch = load(r)
        coh, imu_mods = coherence(A, ch)
        print(f'{r}: modules used {len(set(A.argmax(1)))}, mean max-prob {A.max(1).mean():.2f}, '
              f'joints with cmd/enc/cur together {coh:.2f}, accelerometer split over {imu_mods} module(s)')
        hs.append(A.argmax(1))
    for i in range(len(hs)):
        for j in range(i + 1, len(hs)):
            print(f'ARI {runs[i]} vs {runs[j]}: {adjusted_rand_score(hs[i], hs[j]):.3f}')
