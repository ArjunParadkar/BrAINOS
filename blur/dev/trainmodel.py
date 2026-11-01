"""
trainmodel.py -- the BLUR MJCF, edited in memory into something MJX can run fast.

What changes, and why it is allowed:
  * collision cylinders -> capsules of the same radius and half-length
    (MJX has no cylinder-box / cylinder-cylinder; for a body hitting a floor
    the rounded ends matter only at the rims);
  * the tail-tip gripper's fingers stop colliding (irrelevant to locomotion);
  * every collision geom except the feet and a short list of torso/head/knee
    geoms that a falling body lands on is turned off (contacts are the cost);
  * actuators become plain torque motors (the impedance law is the policy's
    action space, applied in the env exactly as the node firmware does);
  * the 0.5 ms timestep can be raised for training (checked for stability).
The meshes, masses, inertias, joints, ranges, springs and tendons are the
MJCF's, untouched. Policies trained here are re-validated on the full model
(blur/sim/nerves.py) before any number is reported.
"""
import json, os, sys
import mujoco

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import blurcfg  # noqa: E402  (per-machine MJCF location, sha-checked)
DESC = blurcfg.load_descriptor()

# bodies whose collision geoms stay on: the falling body lands on these
KEEP_BODIES = ('chest', 'pelvis', 'head')


def build(scene='flat', timestep=None, prune=True, pyramidal=False, max_per_body=None):
    spec = mujoco.MjSpec.from_file(os.path.join(DESC['mjcf_dir'], f'scene_{scene}.xml'))
    for g in spec.geoms:
        collides = g.contype != 0 or g.conaffinity != 0
        if not collides:
            continue
        body = g.parent.name if g.parent is not None else ''
        is_world = body in ('', 'world')
        if is_world:
            continue
        cls = g.classname.name if g.classname is not None else ''
        if cls == 'grip' or body.startswith('finger'):
            g.contype = 0; g.conaffinity = 0
            continue
        if g.type == mujoco.mjtGeom.mjGEOM_CYLINDER:
            g.type = mujoco.mjtGeom.mjGEOM_CAPSULE
        if g.type == mujoco.mjtGeom.mjGEOM_ELLIPSOID:
            g.type = mujoco.mjtGeom.mjGEOM_SPHERE
            g.size[0] = max(g.size[0], g.size[1], g.size[2])
        if prune and cls != 'foot' and not any(body.startswith(k) for k in KEEP_BODIES):
            g.contype = 0; g.conaffinity = 0
        elif pyramidal:
            g.condim = 3
    if pyramidal:
        spec.option.cone = mujoco.mjtCone.mjCONE_PYRAMIDAL
    m = spec.compile()
    for i in range(m.nu):
        m.actuator_gainprm[i, :] = 0; m.actuator_gainprm[i, 0] = 1.0
        m.actuator_biasprm[i, :] = 0
    if timestep:
        m.opt.timestep = timestep
    return m


if __name__ == '__main__':
    import collections, sys
    m = build()
    on = [g for g in range(m.ngeom) if m.geom_contype[g] or m.geom_conaffinity[g]]
    print('colliding geoms:', len(on), collections.Counter(int(m.geom_type[g]) for g in on))
    print('bodies:', sorted({m.body(m.geom_bodyid[g]).name for g in on}))
