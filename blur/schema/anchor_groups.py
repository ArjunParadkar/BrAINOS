#!/usr/bin/env python3
"""anchor_groups.py -- a deterministic, topology-free grouping of the body's
channels to check learned assignments against: affinity(i, j) = how much
channel i's past 0.64 s linearly predicts channel j 200 ms ahead beyond j's
own past (symmetrised), then spectral clustering into K groups."""
import os, sys, json
os.environ.setdefault('OMP_NUM_THREADS', '1')
import numpy as np, torch
from sklearn.cluster import SpectralClustering
from schema_model import load, WIN

X, ch, group, gt, info = load(sys.argv[1]); K = int(sys.argv[2]) if len(sys.argv) > 2 else 12
N, C = X.shape; H = 10
dev = 'cuda' if torch.cuda.is_available() else 'cpu'
Xt = torch.as_tensor(X, device=dev)
idx = torch.arange(WIN - 1, N - H, 3, device=dev)
win = idx[:, None] + torch.arange(-WIN + 1, 1, device=dev)[None, :]
Y = Xt[idx + H]                                     # (n, C)
def r2(F, y):
    F = torch.cat([F, torch.ones(len(F), 1, device=dev)], 1)
    sol = torch.linalg.lstsq(F, y).solution
    res = ((y - F @ sol) ** 2).sum(0); tot = ((y - y.mean(0)) ** 2).sum(0)
    return 1 - res / tot
own = torch.stack([r2(Xt[win][:, :, j], Y[:, j:j + 1])[0] for j in range(C)])
gain = torch.zeros(C, C, device=dev)
for i in range(C):
    for j in range(C):
        if i == j: continue
        F = torch.cat([Xt[win][:, :, j], Xt[win][:, :, i]], 1)
        gain[i, j] = (r2(F, Y[:, j:j + 1])[0] - own[j]).clamp(min=0)
G = gain.cpu().numpy(); Aff = G + G.T; np.fill_diagonal(Aff, 0)
lab = SpectralClustering(K, affinity='precomputed', random_state=0).fit_predict(Aff + 1e-6)
np.savez('runs/anchor.npz', labels=lab, affinity=Aff, channels=np.array(ch))
for k in range(K):
    print(f'g{k:2d}:', ' '.join(c for c, l in zip(ch, lab) if l == k))
