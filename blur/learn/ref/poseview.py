#!/usr/bin/env python3
"""poseview.py -- render named poses kinematically (no dynamics): a contact sheet
for designing keyframes.   python poseview.py out.png 'name:J=v,J=v;...' [--rpy r,p,y] [--z z]"""
import os, sys, math
os.environ.setdefault('MUJOCO_GL', 'egl')
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, os.path.dirname(HERE))
import numpy as np, mujoco, imageio
import env as E
from tasks import quat_from_rpy


def render_poses(poses, out, rpy=(0, 0, 0), z=0.35, views=(0, 90)):
    m = mujoco.MjModel.from_xml_path(os.path.join(E.DESC['mjcf_dir'], 'scene_flat.xml'))
    d = mujoco.MjData(m)
    key = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_KEY, 'stand')
    chest = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'chest')
    rr = mujoco.Renderer(m, 300, 400)
    rows = []
    for name, q in poses:
        mujoco.mj_resetDataKeyframe(m, d, key)
        for i, a in enumerate(E.A):
            if not a['tendon']:
                j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a['target'])
                d.qpos[m.jnt_qposadr[j]] = q[i]
        d.qpos[2] = z; d.qpos[3:7] = quat_from_rpy(*rpy)
        mujoco.mj_forward(m, d)
        lo = min(d.geom_xpos[g][2] for g in range(m.ngeom) if m.geom_bodyid[g] > 0)
        imgs = []
        for az in views:
            c = mujoco.MjvCamera(); c.type = mujoco.mjtCamera.mjCAMERA_FREE
            c.lookat[:] = d.xpos[chest]; c.distance = 0.9; c.elevation = -10; c.azimuth = az
            rr.update_scene(d, c); im = rr.render().copy()
            imgs.append(im)
        row = np.concatenate(imgs, 1)
        rows.append(row)
        print(f'{name}: chest z {d.xpos[chest][2]:.3f}, lowest geom centre {lo:.3f}')
    imageio.imwrite(out, np.concatenate(rows, 0))


def parse(spec):
    poses = []
    for item in spec.split(';'):
        name, _, kv = item.partition(':')
        q = E.QSTAND.copy()
        for p in filter(None, kv.split(',')):
            k, v = p.split('='); q[E.NAMES.index(k)] = float(v)
        poses.append((name, q))
    return poses


if __name__ == '__main__':
    rpy = (0, 0, 0); z = 0.35
    if '--rpy' in sys.argv: rpy = tuple(map(float, sys.argv[sys.argv.index('--rpy') + 1].split(',')))
    if '--z' in sys.argv: z = float(sys.argv[sys.argv.index('--z') + 1])
    render_poses(parse(sys.argv[2]), sys.argv[1], rpy, z)
