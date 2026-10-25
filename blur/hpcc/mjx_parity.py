#!/usr/bin/env python3
"""
mjx_parity.py -- does MJX (JAX, GPU) reproduce CPU MuJoCo on BLUR v36?

One fixed rollout, run on both engines from the same initial state with the
same target sequence:

  model    blur/dev/trainmodel.build(): the v36 MJCF with cylinders->capsules,
           ellipsoids->spheres, collisions only on feet/chest/pelvis/head
           (MJX cannot collide cylinders with boxes, so the full MJCF does not
           load in MJX at all). Actuators = the node impedance law as MuJoCo
           position servos (kp = KP_SCALE x deliverable torque, kd = 0.04 kp,
           force clamp = deliverable torque), exactly env.py's stand gains.
           No speed derate (it rewrites the model every 5 ms; same in both).
  rollout  'walk': 1 s stand, 0.3 s blend, then the rig's walk cycle
           (moves.npz) as open-loop joint targets; 'drop': stand targets,
           body released 12 cm up with 25 deg roll and 10 deg pitch (impact,
           bounce, settle). Contacts with the floor throughout.
  ref      CPU MuJoCo, float64, the model's own solver settings (Newton,
           100 iterations / 50 line-search).

Reported per variant: per-joint max/RMS error of the 26 actuated joint
coordinates over time, base position/orientation error, per-foot normal
ground-reaction-force error, and divergence times (first time the max joint
error exceeds 0.01 / 0.1 rad, base error exceeds 1 cm). Two CPU-only
references set the scale: the chaos floor (CPU vs CPU with the initial
velocity perturbed by 1e-9 / 1e-6) and the model-simplification gap (full
MJCF vs the training model, both CPU).

  bpy mjx_parity.py --variant f32 --out results/4c        (one JAX config per process)
  bpy mjx_parity.py --variant cpu --out results/4c        (CPU-only references)
"""
import argparse, json, math, os, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
BLUR = os.path.dirname(HERE)
sys.path[:0] = [os.path.join(BLUR, 'learn'), os.path.join(BLUR, 'dev'), BLUR]

VARIANTS = {
    # name: dict(x64, dt, iterations, ls_iterations, tolerance, impl, feetonly)
    # MJX's Newton solver runs until `tolerance` (MuJoCo default 1e-8) or the
    # iteration cap; float32 cannot reach 1e-8, so f32 at the default always
    # pays the full 100 x 50 iterations. `tol6` is the float32-reachable stop.
    'f32_fast':   dict(x64=False, dt=0.001, it=5, ls=8),        # a typical MJX training budget
    'f32_tol6':   dict(x64=False, dt=0.001, tol=1e-6),
    'f64':        dict(x64=True, dt=0.001),
    'f64_dt05':   dict(x64=True, dt=0.0005),
    'f32_tol6_dt05': dict(x64=False, dt=0.0005, tol=1e-6),
    'f32_fast_feet': dict(x64=False, dt=0.001, it=5, ls=8, feetonly=True),   # paws-only collisions (locomotion, no falls)
    'f32_warp':   dict(x64=False, dt=0.001, impl='warp'),
    'f32':        dict(x64=False, dt=0.001),                     # MuJoCo defaults, as-is
}

ap = argparse.ArgumentParser()
ap.add_argument('--variant', required=True, choices=list(VARIANTS) + ['cpu'])
ap.add_argument('--out', default='results/4c')
ap.add_argument('--scenarios', default='walk,drop')
args = ap.parse_args()

if args.variant != 'cpu' and VARIANTS[args.variant].get('x64'):
    import jax
    jax.config.update('jax_enable_x64', True)

import numpy as np
import mujoco
import env as E
import trainmodel as TM

FEET = ('FR', 'FL', 'RR', 'RL')
_TS = float(os.environ.get('BLUR_PARITY_TSCALE', '1'))   # <1 only for quick laptop checks
T_WALK, T_DROP = 5.0 * _TS, 3.0 * _TS


def build(dt, full=False, iters=None, ls=None, tol=None, feetonly=False):
    if full:
        m = mujoco.MjModel.from_xml_path(os.path.join(E.DESC['mjcf_dir'], 'scene_flat.xml'))
        m.opt.timestep = dt
    else:
        m = TM.build(scene='flat', timestep=dt)
    kp, kd = E.stand_kp()
    act = np.array([mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_ACTUATOR, n) for n in E.NAMES])
    for i in range(E.NU):
        a = act[i]
        m.actuator_gainprm[a, :] = 0; m.actuator_gainprm[a, 0] = kp[i]
        m.actuator_biasprm[a, :] = 0; m.actuator_biasprm[a, 1] = -kp[i]; m.actuator_biasprm[a, 2] = -kd[i]
        m.actuator_forcelimited[a] = 1
        m.actuator_forcerange[a] = [-E.TAU[i], E.TAU[i]]
    if feetonly:
        # only the paws collide: the torso's 47 boxes are 188 of the 233 MJX contact
        # slots (box-plane = 4 each). Locomotion-only; a fall can't be simulated
        for g in range(m.ngeom):
            if m.geom_bodyid[g] != 0 and '_paw' not in m.body(m.geom_bodyid[g]).name:
                m.geom_contype[g] = 0; m.geom_conaffinity[g] = 0
    if tol:
        m.opt.tolerance = tol
    if iters:
        m.opt.iterations = iters
    if ls:
        m.opt.ls_iterations = ls
    return m, act


def coords(m):
    """qpos/qvel addresses (or tendon ids) of the 26 actuated coordinates"""
    qadr, dadr, ten = np.zeros(E.NU, int), np.zeros(E.NU, int), np.full(E.NU, -1)
    for i, a in enumerate(E.A):
        if a['tendon']:
            ten[i] = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_TENDON, a['target'])
        else:
            j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a['target'])
            qadr[i], dadr[i] = m.jnt_qposadr[j], m.jnt_dofadr[j]
    return qadr, dadr, ten


def foot_geoms(m):
    """foot index (0..3) per geom, -1 if not a foot geom"""
    out = np.full(m.ngeom, -1)
    for g in range(m.ngeom):
        b = m.body(m.geom_bodyid[g]).name
        for k, f in enumerate(FEET):
            if b.startswith(f + '_paw') and (m.geom_contype[g] or m.geom_conaffinity[g]):
                out[g] = k
    return out


def initial(m, scenario):
    d = mujoco.MjData(m)
    key = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_KEY, 'stand')
    mujoco.mj_resetDataKeyframe(m, d, key)
    if scenario == 'drop':
        r, p = math.radians(25), math.radians(10)
        qr = np.array([math.cos(r / 2), math.sin(r / 2), 0, 0]); qp = np.array([math.cos(p / 2), 0, math.sin(p / 2), 0])
        q = np.zeros(4); mujoco.mju_mulQuat(q, qp, qr)
        q0 = d.qpos[3:7].copy(); qq = np.zeros(4); mujoco.mju_mulQuat(qq, q, q0)
        d.qpos[2] += 0.12; d.qpos[3:7] = qq
    mujoco.mj_forward(m, d)
    return d


def targets(m, act, scenario, dt):
    """[steps, nu] position targets in actuator-id order"""
    n = int(round((T_WALK if scenario == 'walk' else T_DROP) / dt))
    stand = np.clip(E.QSTAND, E.QMIN, E.QMAX)
    tg = np.tile(stand, (n, 1))
    if scenario == 'walk':
        z = np.load(os.path.join(E.DESC['mjcf_dir'], 'moves.npz'), allow_pickle=True)
        jn = list(z['joint_names']); wq = z['walk__q']; wdt = float(z['walk__dt'])
        idx = [jn.index(a['target']) if a['target'] in jn else -1 for a in E.A]
        for s in range(n):
            t = s * dt
            if t < 1.0 * _TS:
                continue
            f = wq[int((t - 1.0 * _TS) / wdt) % len(wq)]
            w = np.array([f[j] if j >= 0 else stand[i] for i, j in enumerate(idx)])
            b = min(1.0, (t - 1.0 * _TS) / 0.3)
            tg[s] = stand + b * (w - stand)
        tg = np.clip(tg, E.QMIN, E.QMAX)
    out = np.zeros((n, m.nu))
    out[:, act] = tg
    return out


# ------------------------------------------------------------------ CPU
def cpu_rollout(m, d, tg, qvel_eps=0.0, seed=0):
    d = mujoco.MjData(m) if d is None else d
    if qvel_eps:
        d.qvel[:] += np.random.default_rng(seed).normal(0, qvel_eps, m.nv)
    qadr, dadr, ten = coords(m)
    fg = foot_geoms(m)
    n = len(tg)
    Q = np.zeros((n, E.NU)); B = np.zeros((n, 7)); F = np.zeros((n, 4)); S = np.zeros((n, m.nsensordata))
    f6 = np.zeros(6)
    for s in range(n):
        d.ctrl[:] = tg[s]
        mujoco.mj_step(m, d)
        q = d.qpos[qadr].copy(); t = ten >= 0; q[t] = d.ten_length[ten[t]]
        Q[s] = q; B[s] = d.qpos[:7]; S[s] = d.sensordata
        for c in range(d.ncon):
            con = d.contact[c]
            k = max(fg[con.geom1], fg[con.geom2])
            if k >= 0 and con.efc_address >= 0:
                mujoco.mj_contactForce(m, d, c, f6)
                F[s, k] += f6[0]
    return dict(q=Q, base=B, grf=F, sens=S)


# ------------------------------------------------------------------ MJX
def mjx_rollout(m, d0, tg, impl):
    import jax, jax.numpy as jp
    from mujoco import mjx
    assert m.opt.cone == mujoco.mjtCone.mjCONE_ELLIPTIC
    mx = mjx.put_model(m, impl=impl)
    dx = mjx.put_data(m, d0, impl=impl)
    # under x64, step() returns int64 index arrays where put_data gave int32: match the carry
    shapes = jax.eval_shape(mjx.step, mx, dx)
    dx = jax.tree_util.tree_map(lambda a, s: a.astype(s.dtype) if hasattr(a, 'astype') else a, dx, shapes)
    qadr, dadr, ten = coords(m)
    tmask = jp.asarray(ten >= 0); qadr_j = jp.asarray(qadr); ten_j = jp.asarray(np.maximum(ten, 0))
    fg = jp.asarray(foot_geoms(m))

    def grf(d):
        if impl != 'jax':
            return jp.zeros(4)
        # elliptic cone: a contact's first efc row is its normal force
        # (support.contact_force slices with the address, which can't trace)
        c = d._impl.contact
        fn = jp.take(d._impl.efc_force, jp.maximum(c.efc_address, 0))
        k = jp.maximum(fg[c.geom1], fg[c.geom2])
        fn = jp.where((k >= 0) & (c.efc_address >= 0), fn, 0.0)
        return jax.ops.segment_sum(fn, jp.maximum(k, 0), num_segments=4) * 1.0

    def body(d, u):
        d = d.replace(ctrl=u)
        d = mjx.step(mx, d)
        tl = d.ten_length if impl == 'jax' else d.ten_length
        q = jp.where(tmask, tl[ten_j], d.qpos[qadr_j])
        return d, (q, d.qpos[:7], grf(d), d.sensordata)

    roll = jax.jit(lambda d, u: jax.lax.scan(body, d, u))
    u = jp.asarray(tg, dtype=dx.qpos.dtype)
    t0 = time.time(); out = roll(dx, u); jax.block_until_ready(out); t_first = time.time() - t0
    t0 = time.time(); out = roll(dx, u); jax.block_until_ready(out); t_run = time.time() - t0
    _, (Q, B, F, S) = out
    return dict(q=np.asarray(Q, np.float64), base=np.asarray(B, np.float64), grf=np.asarray(F, np.float64),
                sens=np.asarray(S, np.float64), compile_s=t_first - t_run, run_s=t_run)


# ------------------------------------------------------------------ compare
def quat_angle(a, b):
    dot = np.abs(np.sum(a * b, axis=-1) / (np.linalg.norm(a, axis=-1) * np.linalg.norm(b, axis=-1)))
    return 2 * np.arccos(np.clip(dot, 0, 1))


def compare(ref, x, dt, weight):
    eq = np.abs(x['q'] - ref['q'])                       # [steps, 26]
    emax_t = eq.max(axis=1)
    ep = np.linalg.norm(x['base'][:, :3] - ref['base'][:, :3], axis=1)
    eo = quat_angle(x['base'][:, 3:7], ref['base'][:, 3:7])
    ef = np.abs(x['grf'] - ref['grf'])
    t = (np.arange(len(eq)) + 1) * dt

    def first(mask):
        i = np.argmax(mask)
        return float(t[i]) if mask.any() else None

    win = {}
    for w in (0.25, 0.5, 1.0, 2.0, None):
        k = len(eq) if w is None else min(len(eq), int(round(w / dt)))
        win['all' if w is None else f'{w}s'] = dict(
            joint_max=float(eq[:k].max()), joint_rms=float(np.sqrt((eq[:k] ** 2).mean())),
            base_pos_max_m=float(ep[:k].max()), base_ori_max_deg=float(np.degrees(eo[:k].max())),
            grf_rms_N=float(np.sqrt((ef[:k] ** 2).mean())), grf_max_N=float(ef[:k].max()))
    per_joint = {n: dict(max=float(eq[:, i].max()), rms=float(np.sqrt((eq[:, i] ** 2).mean())))
                 for i, n in enumerate(E.NAMES)}
    # foot touch sensors (if the engine fills them)
    return dict(
        windows=win, per_joint=per_joint,
        diverge_joint_0p01_s=first(emax_t > 0.01), diverge_joint_0p1_s=first(emax_t > 0.1),
        diverge_base_1cm_s=first(ep > 0.01),
        grf_ref_mean_total_N=float(ref['grf'].sum(axis=1).mean()), body_weight_N=weight,
        final_base_z_ref=float(ref['base'][-1, 2]), final_base_z=float(x['base'][-1, 2]),
        finite=bool(np.isfinite(x['q']).all()))


def main():
    os.makedirs(args.out, exist_ok=True)
    scen = args.scenarios.split(',')
    res = dict(variant=args.variant, mujoco=mujoco.__version__, scenarios={})
    if args.variant == 'cpu':
        for dt in (0.001, 0.0005):
            for sc in scen:
                m, act = build(dt)
                tg = targets(m, act, sc, dt)
                w = float(mujoco.mj_getTotalmass(m) * 9.81)
                ref = cpu_rollout(m, initial(m, sc), tg)
                r = {}
                for eps in (1e-9, 1e-6):
                    x = cpu_rollout(m, initial(m, sc), tg, qvel_eps=eps)
                    r[f'chaos_qvel{eps:g}'] = compare(ref, x, dt, w)
                mf, actf = build(dt, full=True)
                x = cpu_rollout(mf, initial(mf, sc), targets(mf, actf, sc, dt))
                r['full_vs_train_model'] = compare(ref, x, dt, w)
                mn, actn = build(dt, feetonly=True)
                x = cpu_rollout(mn, initial(mn, sc), targets(mn, actn, sc, dt))
                r['feetonly_vs_train_model'] = compare(ref, x, dt, w)
                t0 = time.time(); cpu_rollout(m, initial(m, sc), tg); r['cpu_wall_s'] = time.time() - t0
                r['steps'] = len(tg)
                res['scenarios'][f'{sc}@{dt}'] = r
                print(sc, dt, {k: (v['diverge_joint_0p1_s'], v['windows']['1.0s']['joint_max']) for k, v in r.items() if isinstance(v, dict)}, flush=True)
    else:
        import jax
        V = VARIANTS[args.variant]
        x64, dt, iters, ls, tol, impl, feetonly = (V.get('x64', False), V['dt'], V.get('it'), V.get('ls'), V.get('tol'),
                                                V.get('impl', 'jax'), V.get('feetonly', False))
        res.update(jax=jax.__version__, device=str(jax.devices()[0]), x64=x64, dt=dt, iterations=iters or 100,
                   ls_iterations=ls or 50, tolerance=tol or 1e-8, impl=impl, feetonly=feetonly)
        for sc in scen:
            mref, act = build(dt, feetonly=feetonly)            # CPU reference: same contacts, MuJoCo's solver defaults
            tg = targets(mref, act, sc, dt)
            ref = cpu_rollout(mref, initial(mref, sc), tg)
            m, _ = build(dt, iters=iters, ls=ls, tol=tol, feetonly=feetonly)
            try:
                x = mjx_rollout(m, initial(m, sc), tg, impl)
            except Exception as e:
                res['scenarios'][sc] = dict(error=repr(e)[:2000]); print(sc, 'ERROR', repr(e)[:500], flush=True); continue
            c = compare(ref, x, dt, float(mujoco.mj_getTotalmass(m) * 9.81))
            c.update(compile_s=x['compile_s'], run_s=x['run_s'], steps=len(tg))
            ts = slice(m.sensor_adr[3], m.sensor_adr[3] + 4)       # FR/FL/RR/RL touch
            c['touch_rms_N'] = float(np.sqrt(((x['sens'][:, ts] - ref['sens'][:, ts]) ** 2).mean()))
            c['touch_ref_mean_N'] = float(ref['sens'][:, ts].sum(axis=1).mean())
            res['scenarios'][sc] = c
            print(sc, args.variant, 'div0.1', c['diverge_joint_0p1_s'], 'div0.01', c['diverge_joint_0p01_s'],
                  '1s max', round(c['windows']['1.0s']['joint_max'], 5), 'all max', round(c['windows']['all']['joint_max'], 4), flush=True)
    path = os.path.join(args.out, f'parity_{args.variant}.json')
    json.dump(res, open(path, 'w'), indent=1)
    print('wrote', path)


if __name__ == '__main__':
    main()
