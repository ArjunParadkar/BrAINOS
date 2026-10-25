#!/usr/bin/env python3
"""
smoke.py -- does the BLUR stack import and step on this machine?

  1. versions + JAX devices
  2. descriptor + MJCF found and sha-checked (blurcfg)
  3. CPU MuJoCo: the full v36 MJCF, 1 s at 0.5 ms; the RL env (env.BlurEnv,
     standup task) reset + 25 policy steps; the torch MLP the PPO uses
  4. MJX: the training model on the default JAX device, 64 envs x 100 steps

Prints PASS/FAIL per item and exits non-zero on any failure.
"""
import os, sys, time, traceback

HERE = os.path.dirname(os.path.abspath(__file__))
BLUR = os.path.dirname(HERE)
sys.path[:0] = [os.path.join(BLUR, 'learn'), os.path.join(BLUR, 'dev'), BLUR]
fails = []


def item(name):
    def deco(f):
        t0 = time.time()
        try:
            out = f()
            print(f'PASS {name} ({time.time() - t0:.1f}s) {out or ""}', flush=True)
        except Exception:
            fails.append(name)
            print(f'FAIL {name}\n{traceback.format_exc()}', flush=True)
    return deco


@item('versions')
def _():
    import numpy, scipy, mujoco, jax, cma, imageio, torch
    return dict(python=sys.version.split()[0], numpy=numpy.__version__, scipy=scipy.__version__,
                mujoco=mujoco.__version__, jax=jax.__version__, torch=torch.__version__,
                devices=[str(d) for d in jax.devices()])


@item('descriptor')
def _():
    import blurcfg
    d = blurcfg.load_descriptor()
    return dict(mjcf_dir=d['mjcf_dir'], sha=d['mjcf_sha256'][:12])


@item('cpu full MJCF 1 s')
def _():
    import mujoco, blurcfg
    m = mujoco.MjModel.from_xml_path(blurcfg.mjcf('scene_flat.xml'))
    d = mujoco.MjData(m)
    mujoco.mj_resetDataKeyframe(m, d, 0)
    mujoco.mj_step(m, d, nstep=int(1.0 / m.opt.timestep))
    assert abs(d.qpos[2]) < 1.0 and d.time > 0.99
    return dict(nq=m.nq, nu=m.nu, z=round(float(d.qpos[2]), 4))


@item('cpu RL env + torch MLP')
def _():
    import numpy as np, torch, env as E, skills as S
    e = E.BlurEnv(S.SKILLS['standup'](), seed=0)
    o = e.reset()
    net = torch.nn.Sequential(torch.nn.Linear(len(o), 64), torch.nn.Tanh(), torch.nn.Linear(64, E.NA))
    for _ in range(25):
        with torch.no_grad():
            a = net(torch.as_tensor(o)).numpy()
        o, r, done, _ = e.step(np.tanh(a))
        if done:
            o = e.reset()
    return dict(obs=len(o), r=round(float(r), 3))


@item('mjx 64 envs x 100 steps')
def _():
    import jax, jax.numpy as jp, mujoco
    from mujoco import mjx
    import trainmodel as TM
    m = TM.build(timestep=0.001)
    d = mujoco.MjData(m); mujoco.mj_resetDataKeyframe(m, d, 0); mujoco.mj_forward(m, d)
    mx, dx = mjx.put_model(m), mjx.put_data(m, d)
    b = jax.vmap(lambda v: dx.replace(qvel=dx.qvel + v))(jax.random.normal(jax.random.PRNGKey(0), (64, m.nv)) * 0.01)
    f = jax.jit(lambda b: jax.lax.scan(lambda c, _: (jax.vmap(mjx.step, in_axes=(None, 0))(mx, c), None), b, None, length=100)[0])
    t0 = time.time(); out = f(b); jax.block_until_ready(out.qpos); tc = time.time() - t0
    t0 = time.time(); out = f(out); jax.block_until_ready(out.qpos); tr = time.time() - t0
    assert bool(jp.isfinite(out.qpos).all())
    return dict(device=str(jax.devices()[0]), compile_s=round(tc - tr, 1), steps_per_s=round(6400 / tr))


print('SMOKE', 'FAIL ' + ','.join(fails) if fails else 'PASS', flush=True)
sys.exit(1 if fails else 0)
