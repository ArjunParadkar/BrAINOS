"""ph_validate.py -- the PH pipeline and its pre-registered rule on spaces whose answer is known."""
import json, numpy as np
import schema_ph as S
rng = np.random.default_rng(0)
out = {}
s2 = rng.normal(size=(20000, 3)); s2 /= np.linalg.norm(s2, axis=1, keepdims=True); out['S2 (1,0,1)'] = S.ph(s2, 2, 's2')
ang = rng.uniform(0, 2 * np.pi, 20000); out['S1 (1,1,0)'] = S.ph(np.stack([np.cos(ang), np.sin(ang)], 1), 2, 's1')
a, b = rng.uniform(0, 2 * np.pi, (2, 20000)); out['T2 (1,2,1)'] = S.ph(np.stack([np.cos(a), np.sin(a), np.cos(b), np.sin(b)], 1), 2, 't2')
out['cube3 (1,0,0)'] = S.ph(rng.uniform(size=(20000, 3)), 2, 'cube')
json.dump(out, open('ph_validate.json', 'w'), indent=1); print(json.dumps(out), flush=True)
q = rng.normal(size=(20000, 4)); q /= np.linalg.norm(q, axis=1, keepdims=True)
out['SO3=RP3 Z2:(1,1,1) Z3:(1,0,0)'] = S.ph(S.quat_to_R(q), 2, 'so3')
json.dump(out, open('ph_validate.json', 'w'), indent=1); print(json.dumps(out), flush=True)
