"""Stand-pose search: move the feet (IK on pitch/knee/dist, stand pose in the
nullspace), keep the chest level, and score the static stability margin and
holding torques in the twin. Writes the chosen pose to stance.json."""
import json, math, sys, os
import numpy as np, mujoco
import twin_tools as T
from scipy.optimize import minimize

LEGS = ['FR', 'FL', 'RR', 'RL']

def foot_ids(m):
    return {l: mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SITE, f'{l}_foot') for l in LEGS}

def ik(m, d, q0, targets, iters=200):
    """targets: leg -> world foot position; chest fixed at d.qpos[:7]."""
    fid = foot_ids(m)
    q = d.qpos.copy()
    for _ in range(iters):
        d.qpos[:] = q; mujoco.mj_kinematics(m, d); mujoco.mj_comPos(m, d)
        err_tot = 0
        for l in LEGS:
            js = [mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, f'{l}_{k}') for k in ('pitch', 'knee', 'dist')]
            dofs = [m.jnt_dofadr[j] for j in js]; qa = [m.jnt_qposadr[j] for j in js]
            jp = np.zeros((3, m.nv)); mujoco.mj_jacSite(m, d, jp, None, fid[l])
            J = jp[[0, 2]][:, dofs]                                  # sagittal x, z
            e = (targets[l] - d.site_xpos[fid[l]])[[0, 2]]
            err_tot += float(e @ e)
            Jp = J.T @ np.linalg.inv(J @ J.T + 1e-6 * np.eye(2))
            dq = Jp @ e + (np.eye(3) - Jp @ J) @ (0.2 * (q0[qa] - q[qa]))
            for k, a in enumerate(qa):
                lo, hi = m.jnt_range[js[k]]
                q[a] = np.clip(q[a] + 0.5 * dq[k], lo, hi)
        if err_tot < 1e-8: break
    return q, math.sqrt(err_tot)

def margin(m, d):
    """min distance from the CoM ground projection to the support polygon's edges (m), negative = outside."""
    fid = foot_ids(m)
    P = np.array([d.site_xpos[fid[l]][:2] for l in ['FR', 'RR', 'RL', 'FL']])
    c = d.subtree_com[1][:2]
    best = 1e9
    for i in range(4):
        a, b = P[i], P[(i + 1) % 4]
        e = b - a; n = np.array([-e[1], e[0]]) / np.linalg.norm(e)
        if n @ (P.mean(0) - a) < 0: n = -n
        best = min(best, float(n @ (c - a)))
    return best

def evaluate(params, verbose=False):
    dxf, dxr, dy, dz = params
    t = T.twin(); m, d = t.m, t.d
    st = T.stand_targets()
    q0 = d.qpos.copy()
    fid = foot_ids(m)
    mujoco.mj_kinematics(m, d)
    tg = {}
    for l in LEGS:
        p = d.site_xpos[fid[l]].copy()
        p[0] += dxf if l[0] == 'F' else -dxr
        p[1] += dy * (1 if p[1] > 0 else -1)
        p[2] -= 0.0
        tg[l] = p
    # lower/raise the chest by dz: same as moving the feet down/up relative to it
    for l in LEGS: tg[l][2] += dz
    q, err = ik(m, d, q0, tg)
    tgt = dict(st)
    for n in t.nodes:
        if not n.tendon and n.name[:2] in LEGS and not n.name.endswith('abd'):
            tgt[n.name] = float(q[n.qadr])
    t2 = T.twin(); m2, d2 = t2.m, t2.d
    for n in t2.nodes:
        if n.name in tgt and not n.tendon: d2.qpos[n.qadr] = tgt[n.name]
    d2.qpos[2] -= dz
    mujoco.mj_forward(m2, d2)
    T.hold(t2, tgt, ms=1500)
    mg = margin(m2, d2)
    taus = {n.name: abs(float(d2.actuator_force[n.act])) for n in t2.nodes if not n.tendon}
    worst = max(taus, key=taus.get)
    tilt = T.tilt_deg(t2)
    score = -mg + 0.02 * max(0, taus[worst] - 1.5) + 0.01 * tilt + (10 if err > 0.003 else 0)
    if verbose:
        return dict(params=list(map(float, params)), margin=mg, worst_tau=(worst, taus[worst]), tilt=tilt, ik_err=err,
                    z=T.chest_z(t2), targets=tgt)
    return score

if __name__ == '__main__':
    base = evaluate([0, 0, 0, 0], verbose=True)
    print('rig stand: margin %.3f m, worst holding %s %.2f N*m, tilt %.1f' % (base['margin'], *base['worst_tau'], base['tilt']))
    res = minimize(evaluate, [0.03, 0.0, 0.0, 0.0], method='Nelder-Mead',
                   options=dict(maxfev=int(sys.argv[1]) if len(sys.argv) > 1 else 80, xatol=2e-3, fatol=1e-4, initial_simplex=None))
    best = evaluate(res.x, verbose=True)
    print('best: dx_front %+.3f dx_rear %+.3f dy %+.3f dz %+.3f -> margin %.3f m, worst holding %s %.2f N*m, tilt %.1f, z %.3f'
          % (*best['params'], best['margin'], *best['worst_tau'], best['tilt'], best['z']))
    json.dump(best, open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'stance.json'), 'w'), indent=1)
