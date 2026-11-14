"""The mug task: take the brush, wipe the inside of the mug, put the brush back.

`scene()` builds one instance. `demonstrate()` makes the single
demonstration by driving the arm straight from a hand-written list of
waypoints (no seat, no primitives: see `ArmSim.step_teleop`).
`run_recipe()` runs a learned recipe through the real stack: cognition
process, gate, mandate, runner, arm loop. `judge()` is the success test.
"""
import math, os, subprocess, tempfile
import numpy as np
import arm_sim as A

DEMO = dict(radius=0.035, height=0.05, mug_r=0.150, mug_deg=28.0, mug_yaw=0.0,
            brush_r=0.150, brush_deg=-38.0)


def polar(r, deg, z=0.0):
    return (r * math.cos(math.radians(deg)), r * math.sin(math.radians(deg)), z)


def scene(c, seed=0, style='fluid', teleop=False, record=False):
    mug_at, brush_at = polar(c['mug_r'], c['mug_deg']), polar(c['brush_r'], c['brush_deg'])
    xml = (A.mug('mug', mug_at, c['radius'], c['height'], yaw=c['mug_yaw'],
                 mass=c.get('mass', 0.30), friction=c.get('friction', 1.0))
           + A.brush('brush', brush_at) + A.pad('holder', brush_at))

    def places(sim):
        out = []
        b = sim.pos('brush')
        if sim.upright('brush', 35.0):
            out.append(('brush.grip', (b[0], b[1], b[2] + A.BRUSH_GRIP_Z), sim.yaw('brush'),
                        dict(width=0.012, reach=A.BRUSH_GRIP_Z)))
        out.append(('holder.rest', (brush_at[0], brush_at[1], 0.0), 0.0, {}))
        m = sim.pos('mug')
        out.append(('mug.mouth', (m[0], m[1], m[2] + c['height']), sim.yaw('mug'),
                    dict(radius=c['radius'], depth=c['height'] - A.MUG_FLOOR)))
        return out

    sim = A.ArmSim(xml, places=places, seed=seed, style=style, teleop=teleop, record=record)
    sim.cfg, sim.mug0, sim.pad_at = c, np.array(mug_at), np.array(brush_at)
    sim.probe = wiped
    return sim


def wiped(sim):
    """Which wall cells the bristles reach this tick. Three points up the
    head's axis; each wipes the cell it is level with and pointing at, if
    the head's surface there is within a bristle's length of the wall."""
    if sim.held != sim.body('brush'):
        return
    c = sim.cfg
    mug, Rm = sim.d.xpos[sim.body('mug')], sim.d.xmat[sim.body('mug')].reshape(3, 3)
    b, Rb = sim.d.xpos[sim.body('brush')], sim.d.xmat[sim.body('brush')].reshape(3, 3)
    band_h = (c['height'] - A.MUG_FLOOR) / A.MUG_BANDS
    for frac in (0.15, 0.5, 0.85):
        p = Rm.T @ (b + Rb[:, 2] * (A.BRUSH_HEAD_H * frac) - mug)       # in the mug's own frame
        if not A.MUG_FLOOR <= p[2] <= c['height']:
            continue
        if math.hypot(p[0], p[1]) + A.BRUSH_HEAD_R < c['radius'] - A.BRISTLE_M:
            continue
        band = min(A.MUG_BANDS - 1, int((p[2] - A.MUG_FLOOR) / band_h))
        sector = int(round(math.degrees(math.atan2(p[1], p[0])) % 360.0 / (360.0 / A.MUG_SECTORS))) % A.MUG_SECTORS
        sim.touched.add(f'mug_w{band}_{sector}')


def judge(sim):
    """The pre-registered success test, from the simulator's ground truth."""
    c = sim.cfg
    cells = [f'mug_w{b}_{k}' for b in range(A.MUG_BANDS) for k in range(A.MUG_SECTORS)]
    covered = sum(1 for n in cells if n in sim.touched) / len(cells)
    moved = float(np.linalg.norm(sim.pos('mug')[:2] - sim.mug0[:2]))
    back = float(np.linalg.norm(sim.pos('brush')[:2] - sim.pad_at[:2]))
    upright = sim.upright('mug')
    ok = covered >= 0.8 and moved < 0.03 and upright and back < 0.03 and sim.held < 0
    return dict(ok=bool(ok), covered=covered, mug_moved=moved, brush_from_holder=back, mug_upright=bool(upright),
                released=sim.held < 0, peak_tool_n=sim.peak_tool_n)


def _smooth(u):
    return u * u * (3 - 2 * u)


def demonstrate(path, c=DEMO):
    """One demonstration, written to `path`. Returns the judge's verdict on
    the demonstration itself."""
    sim = scene(c, teleop=True)
    sim.run_settle = None
    bx, by, _ = polar(c['brush_r'], c['brush_deg'])
    mx, my, _ = polar(c['mug_r'], c['mug_deg'])
    H, R, g = c['height'], c['radius'], A.BRUSH_GRIP_Z
    carry = H + 0.06 + g
    depth = H - A.MUG_FLOOR
    high, deep = H - 0.35 * depth + g, H - 0.7 * depth + g
    ring = R - A.BRUSH_HEAD_R + 0.002
    way = [  # (x, y, z, opening, seconds)
        (bx, by, g + 0.06, 0.020, 1.0), (bx, by, g, 0.020, 0.8), (bx, by, g, 0.000, 0.6),
        (bx, by, g + 0.06, 0.000, 0.8), (mx, my, carry, 0.000, 1.4), (mx, my, high, 0.000, 0.9)]
    for level in (high, deep):  # once round near the rim, once round near the floor
        way.append((mx + ring, my, level, 0.000, 0.5))
        for k in range(1, 25):
            a = 2 * math.pi * 1.2 * k / 24
            way.append((mx + ring * math.cos(a), my + ring * math.sin(a), level, 0.000, 0.12))
    way += [(mx, my, deep, 0.000, 0.5), (mx, my, carry, 0.000, 0.9), (bx, by, g + 0.03, 0.000, 1.4),
            (bx, by, g + 0.001, 0.000, 0.8), (bx, by, g + 0.001, 0.020, 0.5), (bx, by, g + 0.06, 0.020, 0.7)]
    lines = []
    here = list(sim.tool_pos()) + [A.Q0[5]]
    for x, y, z, opening, secs in way:
        n = int(secs * 1000)
        for i in range(n):
            u = _smooth((i + 1) / n)
            p = [here[k] + ([x, y, z, opening][k] - here[k]) * u for k in range(4)]
            q = A.solve(p[:3], -90.0, 0.0, p[3])
            assert q is not None, f'the demonstration asks for somewhere out of reach: {p}'
            _, tool_n, grip_n = sim.step_teleop(q)
            if sim.t_us % 10_000 == 0:
                t = sim.tool_pos()
                lines.append('d %d %.5f %.5f %.5f %.5f %.2f %.3f %.3f' % (
                    sim.t_us // 1000, t[0], t[1], t[2], sim.d.qpos[sim.qadr[5]],
                    math.degrees(sim.d.qpos[sim.qadr[4]]), grip_n, tool_n))
                for name, pos, yaw, attrs in sim.places_fn(sim):
                    lines.append('pl %s %.5f %.5f %.5f %.2f %s' % (
                        name, pos[0], pos[1], pos[2], yaw, ' '.join(f'{k}={v:.5f}' for k, v in attrs.items())))
        here = [x, y, z, opening]
    open(path, 'w').write('\n'.join(lines) + '\n')
    verdict = judge(sim)
    sim.close()
    return verdict


def learn(demo_path, recipe_path, without=()):
    args = [A.BIN, 'motor', 'learn', demo_path]
    for k in without:
        args += ['--without', k]
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        return False, r.stderr.strip()
    open(recipe_path, 'w').write(r.stdout)
    return True, r.stderr.strip()


def run_recipe(recipe_path, c, style='fluid', max_ms=45_000, record=False, seed=0, hook=None):
    """Run a recipe on one instance through the whole stack."""
    sim = scene(c, seed=seed, style=style, record=record)
    sim.run(200)                       # let things settle before anything is perceived
    sim.start_mind(recipe_path, stop_go=(style == 'stop-go'))
    done = None
    for _ in range(max_ms):
        sim.step()
        if hook:
            hook(sim)
        done = sim.mind_done()
        if done:
            break
    sim.run(600)                       # let whatever was let go of come to rest
    v = judge(sim)
    v['done'] = done or 'timeout'
    v['ok'] = v['ok'] and done == 'done ok'
    v['ms'] = sim.t_us // 1000
    return v, sim


if __name__ == '__main__':
    import sys, time
    out = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()
    t = time.time()
    v = demonstrate(os.path.join(out, 'demo.log'))
    print('demonstration: %s  (%.1f s wall)' % (v, time.time() - t))
    ok, msg = learn(os.path.join(out, 'demo.log'), os.path.join(out, 'recipe.txt'))
    print('learn:', ok, msg)
    if ok:
        print(open(os.path.join(out, 'recipe.txt')).read())


def sample(seed):
    """A novel instance: mug radius and height each within 30 % of the
    demonstration's, mug and brush anywhere in the part of the table the
    arm can work over, mug turned any way."""
    rng = np.random.default_rng(seed)
    return dict(radius=DEMO['radius'] * rng.uniform(0.7, 1.3), height=DEMO['height'] * rng.uniform(0.7, 1.3),
                mug_r=rng.uniform(0.125, 0.165), mug_deg=rng.uniform(-5.0, 70.0), mug_yaw=rng.uniform(0.0, 360.0),
                brush_r=rng.uniform(0.13, 0.16), brush_deg=rng.uniform(-75.0, -30.0),
                mass=0.30 * rng.uniform(0.7, 1.4), friction=rng.uniform(0.7, 1.2))
