"""Gate E1: the generated MJCF against the reference each body has.

Thresholds are the pre-registered ones (docs/motor/PREREGISTRATION.md).
Single process, a few seconds.

    source motor/pyenv.sh && $MOTOR_PY motor/e1_check.py
"""
import json, math, os, subprocess, sys
import mujoco
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('BRAINOS_BIN', os.path.join(HERE, '..', 'os', 'target', 'release', 'brainos'))
rows, ok_all = [], True
J, S, B = mujoco.mjtObj.mjOBJ_JOINT, mujoco.mjtObj.mjOBJ_SITE, mujoco.mjtObj.mjOBJ_BODY


def row(body, check, measured, threshold, ok):
    global ok_all
    ok_all &= bool(ok)
    rows.append((body, check, measured, threshold, 'PASS' if ok else 'FAIL'))


def generated(body, *flags):
    xml = subprocess.run([BIN, 'motor', 'self-model', body, *flags], check=True,
                         capture_output=True, text=True).stdout
    return mujoco.MjModel.from_xml_string(xml)


def reference(body, n, seed=7):
    out = subprocess.run([BIN, 'motor', 'reference', body, str(n), str(seed)], check=True,
                         capture_output=True, text=True).stdout
    return [json.loads(l) for l in out.splitlines()]


def set_q(m, d, q):
    for name, v in q.items():
        j = mujoco.mj_name2id(m, J, name)
        assert j >= 0, name
        d.qpos[m.jnt_qposadr[j]] = math.radians(v) if m.jnt_type[j] == 3 else v
    # a follower joint is held by an equality the kinematics call does not
    # apply, so place it where the equality would
    for k in range(m.neq):
        if m.eq_type[k] == mujoco.mjtEq.mjEQ_JOINT:
            a, b = m.jnt_qposadr[m.eq_obj1id[k]], m.jnt_qposadr[m.eq_obj2id[k]]
            d.qpos[a] = m.eq_data[k, 1] * d.qpos[b]


def site_err(m, refs, offset=np.zeros(3)):
    d, worst = mujoco.MjData(m), 0.0
    for r in refs:
        mujoco.mj_resetData(m, d)
        set_q(m, d, r['q'])
        mujoco.mj_kinematics(m, d)
        for name, p in r['sites'].items():
            got = d.site_xpos[mujoco.mj_name2id(m, S, name)]
            worst = max(worst, float(np.abs(got - (np.array(p) + offset)).max()))
    return worst


# ---- hexapod ----
refs = reference('hexapod', 1000)
m = generated('hexapod')
height = m.body_pos[mujoco.mj_name2id(m, B, 'body')][2]
up = np.array([0, 0, height])
e = site_err(m, refs, up)
row('hexapod', 'foot position, 1000 poses, as built (shin fixed to thigh) vs LegGeom::foot',
    f'{e*1000:.5f} mm', '<= 0.01 mm', e * 1000 <= 0.01)
# not pre-registered, added after the row above failed, to locate the
# difference: the same reference against a model with the reference's own
# vertical shin, and the built model against rigid-shin kinematics
ma = generated('hexapod-as-stated')
ea = site_err(ma, refs, up)
rows.append(('hexapod', 'post hoc: model with the shin kept vertical vs LegGeom::foot',
             f'{ea*1000:.5f} mm', '(<= 0.01 mm)', 'pass' if ea * 1000 <= 0.01 else 'fail'))
er = site_err(m, [dict(q=r['q'], sites=r['rigid_sites']) for r in refs], up)
rows.append(('hexapod', 'post hoc: as-built model vs rigid-shin kinematics',
             f'{er*1000:.5f} mm', '(<= 0.01 mm)', 'pass' if er * 1000 <= 0.01 else 'fail'))
# how wrong LegGeom::foot is at the lift the tripod gait uses (28 deg)
l = math.radians(28.0)
rows.append(('hexapod', 'post hoc: LegGeom::foot error at the gait lift of 28 deg',
             f'{45*math.sin(l):.1f} mm outward, {45*(1-math.cos(l)):.1f} mm up', 'reported', '-'))

# joint ranges against the servo table, recomputed here from the table's
# own rule (servo = centre + dir * joint, servo in [20, 160], centre 90)
worst = 0.0
for j in range(m.njnt):
    if m.jnt_type[j] != 3:
        continue
    lo, hi = np.degrees(m.jnt_range[j])
    worst = max(worst, abs(lo + 70.0), abs(hi - 70.0))
row('hexapod', 'joint ranges vs servo table', f'{worst:.2e} deg', '<= 1e-3 deg', worst <= 1e-3)

ms = generated('hexapod', '--servo')
d = mujoco.MjData(ms)
pairs = [(mujoco.mj_id2name(ms, J, ms.eq_obj1id[k]), mujoco.mj_id2name(ms, J, ms.eq_obj2id[k]))
         for k in range(ms.neq)]
# hold the body up so the legs swing free, then step the leaders
ms.opt.gravity[:] = 0
for _, lead in pairs:
    a = mujoco.mj_name2id(ms, mujoco.mjtObj.mjOBJ_ACTUATOR, lead)
    d.ctrl[a] = math.radians(20.0)
for _ in range(int(1.0 / ms.opt.timestep)):
    mujoco.mj_step(ms, d)
gap, moved = 0.0, 1e9
for foll, lead in pairs:
    qa = d.qpos[ms.jnt_qposadr[mujoco.mj_name2id(ms, J, foll)]]
    qb = d.qpos[ms.jnt_qposadr[mujoco.mj_name2id(ms, J, lead)]]
    gap, moved = max(gap, abs(math.degrees(qa - qb))), min(moved, abs(math.degrees(qb)))
row('hexapod', f'{len(pairs)} linked swing pairs agree after 1 s',
    f'{gap:.4f} deg (leader moved {moved:.1f} deg)', '<= 0.1 deg, 2 pairs',
    len(pairs) == 2 and gap <= 0.1 and abs(moved - 20) < 1)

# ---- planar arm ----
m = generated('planar-arm')
# statics only: the ground the file carries must not push on the arm
m.opt.disableflags |= mujoco.mjtDisableBit.mjDSBL_CONTACT
refs = reference('planar-arm', 1000)
e = site_err(m, refs)
row('planar arm', 'end effector, 1000 poses', f'{e*1000:.5f} mm', '<= 0.01 mm', e * 1000 <= 0.01)
d, bad, worst_rel = mujoco.MjData(m), 0, 0.0
for r in refs[:200]:
    mujoco.mj_resetData(m, d)
    set_q(m, d, r['q'])
    d.qvel[:] = 0
    d.qacc[:] = 0
    mujoco.mj_inverse(m, d)
    sim, ref = abs(d.qfrc_inverse[0]), r['shoulder_nm']
    err = abs(sim - ref)
    rel = err / max(sim, ref, 1e-9)
    if not (rel <= 0.01 or err <= 0.01):
        bad += 1
    worst_rel = max(worst_rel, rel)
row('planar arm', 'static shoulder torque, 200 poses',
    f'{bad}/200 outside, worst {worst_rel*100:.1f} %', 'within 1 % or 0.01 N·m', bad == 0)
# post hoc: Arm::shoulder_torque_nm adds |m g x| per link, so it is an
# upper bound on the true moment and equals it only with every link on
# one side of the column
over, same_side, same_bad = 0, 0, 0
for r in refs[:200]:
    mujoco.mj_resetData(m, d)
    set_q(m, d, r['q'])
    mujoco.mj_inverse(m, d)
    sim, ref = abs(d.qfrc_inverse[0]), r['shoulder_nm']
    over += sim > ref * 1.001 + 0.01
    mujoco.mj_kinematics(m, d)
    xs = [d.xpos[b][0] for b in range(2, m.nbody)] + [d.site_xpos[0][0]]
    if all(x >= 0 for x in xs) or all(x <= 0 for x in xs):
        same_side += 1
        same_bad += abs(sim - ref) > max(0.01 * max(sim, ref), 0.01)
rows.append(('planar arm', 'post hoc: true moment exceeds the mechanism\'s figure', f'{over}/200',
             '(0)', 'pass' if over == 0 else 'fail'))
rows.append(('planar arm', 'post hoc: agreement when every link is on one side',
             f'{same_side - same_bad}/{same_side} within 1 %', '(all)', 'pass' if same_bad == 0 else 'fail'))

# ---- desk arm (no pre-registered threshold: its reference is new code) ----
m = generated('desk-arm')
e = site_err(m, reference('desk-arm', 1000))
row('desk arm', 'grasp centre, 1000 poses (not a pre-registered check)', f'{e*1000:.5f} mm',
    'reported', True)

# ---- BLUR ----
skel = os.path.join(HERE, 'models', 'blur.skeleton')
desc = json.load(open(os.path.join(HERE, '..', 'blur', 'descriptor.json')))
twin_path = os.path.join(desc['mjcf_dir'], 'scene_flat.xml')
if os.path.exists(skel) and os.path.exists(twin_path):
    g = generated('skeleton:' + skel)
    t = mujoco.MjModel.from_xml_path(twin_path)
    dg, dt = mujoco.MjData(g), mujoco.MjData(t)
    rng = np.random.default_rng(11)
    names = [mujoco.mj_id2name(g, J, j) for j in range(g.njnt) if g.jnt_type[j] == 3]
    followers = {mujoco.mj_id2name(t, J, t.eq_obj1id[k]) for k in range(t.neq)}
    worst = 0.0
    for _ in range(200):
        mujoco.mj_resetData(g, dg)
        mujoco.mj_resetData(t, dt)
        # the same root pose in both, the twin's own
        dg.qpos[:7] = dt.qpos[:7]
        for n in names:
            jt, jg = mujoco.mj_name2id(t, J, n), mujoco.mj_name2id(g, J, n)
            v = rng.uniform(*t.jnt_range[jt])
            dt.qpos[t.jnt_qposadr[jt]] = v
            dg.qpos[g.jnt_qposadr[jg]] = v
        mujoco.mj_kinematics(t, dt)
        mujoco.mj_kinematics(g, dg)
        for b in range(1, g.nbody):
            bt = mujoco.mj_name2id(t, B, mujoco.mj_id2name(g, B, b))
            if bt < 0:
                continue
            worst = max(worst, float(np.abs(dg.xpos[b] - dt.xpos[bt]).max()))
    row('BLUR', f'world position of {g.nbody-1} bodies, 200 configurations',
        f'{worst*1000:.5f} mm', '<= 0.1 mm', worst * 1000 <= 0.1)
    rmax = max(float(np.abs(g.jnt_range[mujoco.mj_name2id(g, J, n)]
                            - t.jnt_range[mujoco.mj_name2id(t, J, n)]).max()) for n in names)
    row('BLUR', f'{len(names)} joint ranges', f'{rmax:.2e} rad', '<= 1e-6', rmax <= 1e-6)
    fmax, count = 0.0, 0
    for a in range(g.nu):
        n = mujoco.mj_id2name(g, mujoco.mjtObj.mjOBJ_ACTUATOR, a)
        for at in range(t.nu):
            if t.actuator_trntype[at] == 0 and mujoco.mj_id2name(t, J, t.actuator_trnid[at, 0]) == n:
                fmax = max(fmax, float(np.abs(g.actuator_ctrlrange[a] - t.actuator_forcerange[at]).max()))
                count += 1
    row('BLUR', f'{count} joint-actuator force ranges (2 tendon-driven omitted)', f'{fmax:.2e} N·m',
        '<= 1e-6', fmax <= 1e-6 and count == 24)
    root = next(i for i in range(1, t.nbody) if t.body_parentid[i] == 0 and t.body_jntnum[i] == 1)
    mt = float(sum(t.body_mass[i] for i in range(1, t.nbody) if t.body_rootid[i] == root))
    mg = float(g.body_mass.sum())
    row('BLUR', 'total mass', f'{mg:.5f} vs {mt:.5f} kg ({abs(mg-mt)/mt*100:.4f} %)', '<= 0.1 %',
        abs(mg - mt) / mt <= 0.001)
else:
    row('BLUR', 'skeleton or twin not on this machine', 'not run', '-', False)

w = [max(len(str(r[i])) for r in rows) for i in range(5)]
for r in rows:
    print('  '.join(str(r[i]).ljust(w[i]) for i in range(5)))
print('\nE1:', 'PASS' if ok_all else 'FAIL')
sys.exit(0 if ok_all else 1)
