#!/usr/bin/env python3
"""export.py -- a trained policy.json -> the BLURMLP1 blob os/blur/src/mlp.rs runs,
plus a parity vector (obs, expected action) the Rust test checks.

  python export.py runs/getup_v1/policy_best.json out.mlp
"""
import json, struct, sys
import numpy as np

def write_blob(pj, path):
    P = json.load(open(pj))
    L = P['layers']
    out = bytearray(b'BLURMLP1')
    out += struct.pack('<III', len(L), P['obs_dim'], P['act_dim'])
    out += struct.pack('<ff', P['obs_clip'], P['action_scale'])
    out += np.asarray(P['obs_mean'], '<f4').tobytes() + np.asarray(P['obs_std'], '<f4').tobytes()
    for l in L:
        w = np.asarray(l['w'], '<f4'); b = np.asarray(l['b'], '<f4')
        out += struct.pack('<II', *w.shape) + w.tobytes() + b.tobytes()
    open(path, 'wb').write(bytes(out))
    return P

def act(P, obs):
    x = np.clip((np.asarray(obs, np.float32) - np.asarray(P['obs_mean'], np.float32)) / np.asarray(P['obs_std'], np.float32),
                -P['obs_clip'], P['obs_clip'])
    for k, l in enumerate(P['layers']):
        x = np.asarray(l['w'], np.float32) @ x + np.asarray(l['b'], np.float32)
        if k + 1 < len(P['layers']):
            x = np.where(x > 0, x, np.exp(x) - 1)
    return np.clip(x, -1, 1)

if __name__ == '__main__':
    P = write_blob(sys.argv[1], sys.argv[2])
    rng = np.random.default_rng(0)
    obs = rng.normal(0, 1, P['obs_dim']).astype(np.float32)
    json.dump(dict(obs=obs.tolist(), act=act(P, obs).tolist()), open(sys.argv[2] + '.parity.json', 'w'))
    print('wrote', sys.argv[2], 'macs', sum(len(l['w']) * len(l['w'][0]) for l in P['layers']))
