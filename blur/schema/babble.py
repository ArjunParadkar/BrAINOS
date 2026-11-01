#!/usr/bin/env python3
"""
babble.py -- B1: motor babbling on a sim stand, logged as the body would log it.

Track B (body schema "inspired by OMEN (Claudi & Fiete)"). During §8
Calibrate the body makes small random movements, the way an infant does,
inside an envelope tighter than KIRA's physical one, while held on a stand.

The stand is a 3-axis GIMBAL that a handler slowly turns through every
orientation (uniformly random target attitudes, 2.5 s minimum-jerk slerps):
without it a body bolted to a fixed stand never sees its own orientation
change, and no orientation map could be learned from the data. The chest is
welded softly to the gimbal; nothing touches the floor.

Logged at 50 Hz (what the body can sense, and what it commanded):
  cmd      26  efference copy: the joint targets sent to the nodes
  q        26  encoders (noise as env.py)
  cur      26  motor current, as torque / deliverable torque (the node's own reading)
  imu       6  BMI270: specific force (3) + rate (3), mounting tilt, bias, noise
  cam   HxWx3  head camera ('eye', added here: the MJCF has no camera), 10 Hz
Ground truth, NEVER given to the model, for evaluation only:
  chest_quat, head_xmat, gimbal_quat

Rig additions (camera, gimbal, landmarks) are made in memory with MjSpec;
the MJCF on disk is untouched. Landmarks are coloured posts at irregular
bearings so the camera can tell headings apart (a bare checker floor is
90-degree symmetric).

  python babble.py out.npz --minutes 10 [--seed 0]
"""
import os, sys, math, argparse, time
os.environ.setdefault('MUJOCO_GL', 'egl')
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'learn'))
import numpy as np
import mujoco
import env as E
from ref.keyframes import min_jerk

CAM_HW = (48, 64)
LOG_HZ = 50
CAM_EVERY = 5                      # 10 Hz
CUR_FRAC = 0.5                     # babbling envelope: half the deliverable torque
Q_MARGIN = 0.1                     # and 10 % in from each end of every joint's range
LANDMARKS = [(0.4, 1.6, 0.9, (0.9, 0.1, 0.1)), (1.7, 1.4, 0.5, (0.1, 0.8, 0.1)), (2.6, 1.8, 1.3, (0.1, 0.2, 0.9)),
             (3.5, 1.5, 0.7, (0.9, 0.8, 0.1)), (4.6, 1.7, 1.1, (0.8, 0.1, 0.8)), (5.5, 1.3, 0.4, (0.1, 0.8, 0.8))]
GIMBAL_Z = 0.85


def build_rig():
    spec = mujoco.MjSpec.from_file(os.path.join(E.DESC['mjcf_dir'], 'scene_flat.xml'))
    head = spec.body('head')
    head.add_camera(name='eye', pos=[0.11, 0, 0.03], xyaxes=[0, -1, 0, 0, 0, 1], fovy=90)
    g = spec.worldbody.add_body(name='gimbal', mocap=True, pos=[0, 0, GIMBAL_Z])
    for k, (bearing, r, h, rgb) in enumerate(LANDMARKS):
        spec.worldbody.add_geom(name=f'lm{k}', type=mujoco.mjtGeom.mjGEOM_BOX, size=[0.08, 0.08, h / 2],
                                pos=[r * math.cos(bearing), r * math.sin(bearing), h / 2],
                                rgba=[*rgb, 1], contype=0, conaffinity=0)
    eq = spec.add_equality()
    eq.type = mujoco.mjtEq.mjEQ_WELD
    eq.objtype = mujoco.mjtObj.mjOBJ_BODY
    eq.name = 'gimbal_weld'; eq.name1 = 'chest'; eq.name2 = 'gimbal'
    eq.solref = [0.02, 1.0]
    return spec.compile()


class NoTask:
    action_mode = 'offset'

    def gains(self, env):
        kp, kd = E.stand_kp()
        return kp, kd

    def nominal(self, env):
        return E.QSTAND

    def command(self, env):
        return np.zeros(0)

    def reset(self, env):
        env.t = 0.0


def random_quat(rng):
    u1, u2, u3 = rng.random(3)
    return np.array([math.sqrt(1 - u1) * math.sin(2 * math.pi * u2), math.sqrt(1 - u1) * math.cos(2 * math.pi * u2),
                     math.sqrt(u1) * math.sin(2 * math.pi * u3), math.sqrt(u1) * math.cos(2 * math.pi * u3)])


def slerp(q0, q1, s):
    d = float(np.dot(q0, q1))
    if d < 0:
        q1, d = -q1, -d
    if d > 0.9995:
        q = q0 + s * (q1 - q0)
        return q / np.linalg.norm(q)
    th = math.acos(d)
    return (math.sin((1 - s) * th) * q0 + math.sin(s * th) * q1) / math.sin(th)


def main():
    p = argparse.ArgumentParser()
    p.add_argument('out'); p.add_argument('--minutes', type=float, default=10.0); p.add_argument('--seed', type=int, default=0)
    a = p.parse_args()
    rng = np.random.default_rng(a.seed)
    m = build_rig()
    env = E.BlurEnv(NoTask(), seed=a.seed, randomize=False, model=m)
    env.randomize_body()
    kp, kd = E.stand_kp()
    env.set_gains(kp, kd)
    d = env.d
    mujoco.mj_resetDataKeyframe(m, d, env.key)
    gid = m.body_mocapid[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'gimbal')]
    d.mocap_pos[gid] = [0, 0, GIMBAL_Z]; d.mocap_quat[gid] = [1, 0, 0, 0]
    d.qpos[0:3] = [0, 0, GIMBAL_Z]; d.qpos[3:7] = [1, 0, 0, 0]
    mujoco.mj_forward(m, d)
    env.push = None; env.track_attitude = True; env.att = None; env.t = 0.0
    env.prev_action = np.zeros(E.NA)
    # the babbling envelope (inside KIRA's physical one)
    lo = E.QMIN + Q_MARGIN * (E.QMAX - E.QMIN); hi = E.QMAX - Q_MARGIN * (E.QMAX - E.QMIN)
    centre = np.clip(E.QSTAND, lo, hi)
    m.actuator_forcerange[env.act, 0] = -CUR_FRAC * E.TAU; m.actuator_forcerange[env.act, 1] = CUR_FRAC * E.TAU
    env.derate = lambda: None
    eye = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_CAMERA, 'eye')
    head = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'head')
    rr = mujoco.Renderer(m, *CAM_HW)
    n = int(a.minutes * 60 * LOG_HZ)
    L = {k: [] for k in ('cmd', 'q', 'cur', 'imu', 'chest_quat', 'head_xmat', 'gimbal_quat')}
    cams = []; cam_idx = []
    u = np.zeros(E.NU)
    tau_ou = 0.4
    q_from = np.array([1.0, 0, 0, 0]); q_to = random_quat(rng); seg_t = 0.0; SEG = 2.5
    t0 = time.time()
    env.apply_targets(centre, 2000)        # warm-up: the weld and the filter settle before logging
    for k in range(n):
        # efference: an Ornstein-Uhlenbeck excursion per joint, inside the envelope
        u += (-u / tau_ou) * 0.02 + math.sqrt(2 * 0.02 / tau_ou) * rng.normal(0, 1, E.NU)
        cmd = np.where(u >= 0, centre + np.tanh(0.8 * u) * (hi - centre), centre + np.tanh(0.8 * u) * (centre - lo))
        # the handler turns the gimbal
        s = min_jerk(seg_t / SEG)
        d.mocap_quat[gid] = slerp(q_from, q_to, s)
        seg_t += 0.02
        if seg_t >= SEG:
            q_from, q_to, seg_t = q_to, random_quat(rng), 0.0
        env.apply_targets(cmd, 20)
        acc, gyr = env.imu_sample()
        L['cmd'].append(cmd); L['q'].append(env.q() + env.rng.normal(0, env.enc_sd, E.NU))
        L['cur'].append(d.actuator_force[env.act] / E.TAU)
        L['imu'].append(np.concatenate([acc, gyr]))
        L['chest_quat'].append(d.xquat[env.chest].copy()); L['head_xmat'].append(d.xmat[head].copy())
        L['gimbal_quat'].append(d.mocap_quat[gid].copy())
        if k % CAM_EVERY == 0:
            rr.update_scene(d, eye); cams.append(rr.render().copy()); cam_idx.append(k)
        if k % 3000 == 0:
            print(f'[{time.strftime("%H:%M:%S")}] {k / LOG_HZ / 60:.1f} sim-min, wall {time.time() - t0:.0f} s', flush=True)
    np.savez_compressed(a.out, **{k: np.asarray(v, np.float32) for k, v in L.items()},
                        cam=np.asarray(cams, np.uint8), cam_idx=np.asarray(cam_idx), names=np.array(E.NAMES),
                        log_hz=LOG_HZ, cur_frac=CUR_FRAC, q_margin=Q_MARGIN, seed=a.seed)
    print('wrote', a.out, n, 'samples,', len(cams), 'frames', flush=True)


if __name__ == '__main__':
    main()
