"""Export the BLUR twin's kinematic skeleton for the self-model generator.

Reads the twin MJCF (read-only), writes motor/models/blur.skeleton: body
tree, frames, masses and inertias, joints with limits, springs and
actuator force limits. What a skeleton cannot carry is written as `omit`
lines: meshes, contact geometry, tendons and tendon-driven actuators.

    $MOTOR_PY motor/blur_skeleton.py [MJCF_DIR]
"""
import json, math, os, sys
import mujoco
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
desc = json.load(open(os.path.join(HERE, '..', 'blur', 'descriptor.json')))
mjcf_dir = sys.argv[1] if len(sys.argv) > 1 else desc['mjcf_dir']
m = mujoco.MjModel.from_xml_path(os.path.join(mjcf_dir, 'scene_flat.xml'))
name = lambda t, i: mujoco.mj_id2name(m, t, i)
B, J = mujoco.mjtObj.mjOBJ_BODY, mujoco.mjtObj.mjOBJ_JOINT

root = next(i for i in range(1, m.nbody) if m.body_parentid[i] == 0 and m.body_jntnum[i] == 1
            and m.jnt_type[m.body_jntadr[i]] == mujoco.mjtJoint.mjJNT_FREE)
# the robot is the subtree under the free-jointed root
keep = [i for i in range(1, m.nbody) if m.body_rootid[i] == root]
index = {b: k for k, b in enumerate(keep)}
effort = {}
tendon_driven = []
for a in range(m.nu):
    if m.actuator_trntype[a] == mujoco.mjtTrn.mjTRN_JOINT:
        effort[m.actuator_trnid[a, 0]] = float(m.actuator_forcerange[a, 1])
    else:
        tendon_driven.append(name(mujoco.mjtObj.mjOBJ_ACTUATOR, a))

out = ['name blur']
f = lambda x: repr(float(x))
for b in keep:
    par = -1 if b == root else index[m.body_parentid[b]]
    pos = m.body_pos[b] if b != root else np.zeros(3)
    quat = m.body_quat[b] if b != root else np.array([1.0, 0, 0, 0])
    out.append(' '.join(['link', name(B, b) or f'b{b}', str(par)] + [f(x) for x in (
        *pos, *quat, m.body_mass[b], *m.body_ipos[b], *m.body_iquat[b], *m.body_inertia[b])]))
    if b != root and m.body_jntnum[b] == 1:
        j = m.body_jntadr[b]
        assert m.jnt_type[j] == mujoco.mjtJoint.mjJNT_HINGE and m.jnt_limited[j]
        deg = 180.0 / math.pi
        out.append(' '.join(['joint', name(J, j)] + [f(x) for x in (
            *m.jnt_axis[j], *m.jnt_pos[j], m.jnt_range[j, 0] * deg, m.jnt_range[j, 1] * deg,
            effort.get(j, 0.0), m.jnt_stiffness[j], m.qpos_spring[m.jnt_qposadr[j]] * deg,
            1.0 if j in effort else 0.0)]))
for e in range(m.neq):
    if m.eq_type[e] == mujoco.mjtEq.mjEQ_JOINT:
        c = m.eq_data[e, :5]
        if abs(c[0]) < 1e-12 and not np.any(np.abs(c[2:]) > 1e-12):
            out.append(f'follow {name(J, m.eq_obj1id[e])} {name(J, m.eq_obj2id[e])} {f(c[1])}')
        else:
            out.append(f'omit joint coupling {name(J, m.eq_obj1id[e])} is not a pure ratio')
out.append(f'omit {m.nmesh} meshes and {m.ngeom} contact geoms: a skeleton has no surfaces')
out.append(f'omit {m.ntendon} tendons and the springs on them')
out.append('omit tendon-driven actuators: ' + ', '.join(tendon_driven))
out.append('omit joint damping, armature and friction loss')
path = os.path.join(HERE, 'models', 'blur.skeleton')
open(path, 'w').write('\n'.join(out) + '\n')
print(f'wrote {path}: {len(keep)} bodies, {sum(1 for l in out if l.startswith("joint"))} joints, '
      f'{len(effort)} joint actuators, mass {m.body_mass[keep].sum():.5f} kg')
