"""PH of chosen modules of a second seed, same pipeline (reproducibility check)."""
import sys, json, numpy as np
import schema_ph as S
L = dict(np.load(sys.argv[1] + '/latents.npz'))
out = {}
for k in map(int, sys.argv[2:]):
    r = S.ph(L['Z'][:, k], 2, f'm{k}')
    out[k] = {p: r[p]['betti'] for p in ('Z2', 'Z3')}
    print(k, out[k], flush=True)
json.dump(out, open(sys.argv[1] + '/ph_seedcheck.json', 'w'))
