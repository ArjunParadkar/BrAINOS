"""Gate E3: each primitive on its own, over randomised objects.

Every trial goes through the whole stack: a one-step (or, for `place`,
two-step) recipe is handed to the cognition process, which asks the seat,
which asks the gate. Objects vary in size (+-30 %), mass (x0.5 to x2),
friction (0.6 to 1.2), position and heading.

    e3_gate.py KIND FIRST LAST      KIND in grasp place push turn
    e3_gate.py report

Test seeds are 0..99; seeds from 1000 were used while writing the scenes.
`wipe` is read from the E4 runs (100 randomised mugs): its own test is
coverage of at least 80 % with the wipe ending inside its mandate.
"""
import json, math, os, sys, tempfile, time
import mujoco
import numpy as np
import arm_sim as A
import e4_gate as E4

RESULTS = os.path.join(E4.OUT, '..', 'e3', 'results.json')
THRESHOLD = dict(grasp=90, place=90, push=85, wipe=85, turn=85, pour=75)


def polar(r, deg, z=0.0):
    return (r * math.cos(math.radians(deg)), r * math.sin(math.radians(deg)), z)


def knob(name, pos, radius, friction):
    """A knob on a post, turning about the vertical with a little drag."""
    return (f'<body name="{name}_base" pos="{pos[0]} {pos[1]} 0">'
            f'<geom name="{name}_post" type="cylinder" size="0.006 0.0125" pos="0 0 0.0125" contype="1" conaffinity="1"/>'
            f'<body name="{name}" pos="0 0 0.04"><joint name="{name}_turn" type="hinge" axis="0 0 1" '
            f'damping="0.004" frictionloss="0.01"/>'
            f'<geom name="{name}_g" type="cylinder" size="{radius} 0.015" mass="0.03" contype="1" conaffinity="1" '
            f'friction="{friction} 0.02 0.0001" rgba="0.7 0.2 0.2 1"/>'
            f'<geom name="{name}_mark" type="box" size="{radius*0.9} 0.001 0.001" pos="0 0 0.0155" contype="0" conaffinity="0" rgba="1 1 1 1"/>'
            f'</body></body>')


def make(kind, seed):
    """One instance: the scene, the recipe text, and how to judge it."""
    rng = np.random.default_rng(seed + {'grasp': 0, 'place': 10_000, 'push': 20_000, 'turn': 30_000}[kind])
    scale, mass, mu = rng.uniform(0.7, 1.3), rng.uniform(0.5, 2.0), rng.uniform(0.6, 1.2)
    r, deg, yaw = rng.uniform(0.13, 0.20), rng.uniform(-60.0, 60.0), rng.uniform(0.0, 360.0)
    at = polar(r, deg)
    c = dict(kind=kind, scale=scale, mass=mass, friction=mu, r=r, deg=deg, yaw=yaw)
    if kind in ('grasp', 'place'):
        hx, hy, hz = 0.013 * scale, 0.018 * scale, 0.016 * scale
        xml = A.block('thing', (at[0], at[1], hz), (hx, hy, hz), mass=0.04 * mass, friction=mu, yaw=yaw)
        grip_z = max(hz, 0.028)
        recipe = 'grasp frame=thing.grip it=thing to= width_m=width*1+0\n'
        goal = None
        if kind == 'place':
            # somewhere else on the table, at least 6 cm away
            while True:
                gr, gdeg = rng.uniform(0.13, 0.20), rng.uniform(-60.0, 60.0)
                goal = polar(gr, gdeg)
                if math.hypot(goal[0] - at[0], goal[1] - at[1]) > 0.06:
                    break
            xml += A.pad('goal', goal)
            recipe += f'place frame=goal.rest it=thing to=goal width_m=width*1+0 tool_m={grip_z - 0.0:.4f}\n'

        def places(sim):
            p = sim.pos('thing')
            out = [('thing.grip', (p[0], p[1], p[2] - hz + grip_z), sim.yaw('thing'), dict(width=2 * hx))]
            if goal:
                out.append(('goal.rest', (goal[0], goal[1], 0.0), 0.0, {}))
            return out

        def judge(sim, done):
            p = sim.pos('thing')
            if kind == 'grasp':
                # held 5 cm up for a second: the standing grant holds it after the primitive ends
                ok = done == 'done ok'
                for _ in range(1000):
                    sim.step()
                    ok = ok and sim.held == sim.body('thing') and sim.pos('thing')[2] - hz >= 0.05
                return dict(ok=bool(ok), lifted=float(sim.pos('thing')[2] - hz))
            d = float(math.hypot(p[0] - goal[0], p[1] - goal[1]))
            up = sim.upright('thing')
            return dict(ok=bool(done == 'done ok' and d <= 0.015 and up and sim.held < 0), off_target=d, upright=bool(up),
                        released=bool(sim.held < 0))
        return c, xml, recipe, places, judge
    if kind == 'push':
        rad, hh = 0.022 * scale, 0.012 * scale
        # the puck starts nearer the arm and is pushed outward, 4 to 9 cm
        r = rng.uniform(0.13, 0.16)
        at = polar(r, deg)
        dist, direction = rng.uniform(0.04, 0.09), deg + rng.uniform(-50.0, 50.0)
        goal = (at[0] + dist * math.cos(math.radians(direction)), at[1] + dist * math.sin(math.radians(direction)), 0.0)
        xml = A.puck('thing', (at[0], at[1], hh), rad, hh, mass=0.05 * mass, friction=mu) + A.pad('goal', goal, half=0.02)
        c.update(dist=dist)
        recipe = f'push frame=thing.side it=thing to=goal radius_m=radius*1+0 distance_m={dist:.4f} height_m={max(0.012, hh):.4f}\n'

        def places(sim):
            p = sim.pos('thing')
            return [('thing.side', (p[0], p[1], 0.0), 0.0, dict(radius=rad)), ('goal.rest', goal, 0.0, {})]

        def judge(sim, done):
            p = sim.pos('thing')
            d = float(math.hypot(p[0] - goal[0], p[1] - goal[1]))
            return dict(ok=bool(done == 'done ok' and d <= 0.02 and sim.upright('thing')), off_target=d,
                        upright=bool(sim.upright('thing')))
        return c, xml, recipe, places, judge
    if kind == 'turn':
        rad = 0.014 * scale
        angle = rng.choice([-1.0, 1.0]) * rng.uniform(30.0, 80.0)
        xml = knob('knob', at, rad, mu)
        c.update(angle=angle)
        recipe = f'turn frame=knob.knob it=knob to= width_m=width*1+0 angle_deg={angle:.2f}\n'

        def places(sim):
            return [('knob.knob', (at[0], at[1], 0.04), 0.0, dict(width=2 * rad))]

        def judge(sim, done):
            j = mujoco.mj_name2id(sim.m, mujoco.mjtObj.mjOBJ_JOINT, 'knob_turn')
            got = math.degrees(sim.d.qpos[sim.m.jnt_qposadr[j]])
            return dict(ok=bool(done == 'done ok' and abs(got - angle) <= 10.0 and sim.held < 0), turned=got, wanted=angle,
                        released=bool(sim.held < 0))
        return c, xml, recipe, places, judge
    raise SystemExit(f'no scene for {kind}')


def trial(kind, seed, verbose=False):
    c, xml, recipe, places, judge = make(kind, seed)
    sim = A.ArmSim(xml, places=places, seed=seed)
    sim.run(200)
    with tempfile.NamedTemporaryFile('w', suffix='.recipe', delete=False) as f:
        f.write(recipe)
    sim.start_mind(f.name)
    done = None
    for _ in range(30_000):
        sim.step()
        done = sim.mind_done()
        if done:
            break
    sim.run(500)
    v = judge(sim, done or 'timeout')
    v.update(done=done or 'timeout', peak_tool_n=float(sim.peak_tool_n),
             ended=[u.split()[3] for u in sim.ups if ' ended ' in u and u.split()[1].isdigit()])
    if verbose:
        print([u for u in sim.ups if not u.startswith('up pose')])
    sim.close()
    os.unlink(f.name)
    return v


def main():
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    r = json.load(open(RESULTS)) if os.path.exists(RESULTS) else {}
    if sys.argv[1] != 'report':
        kind, t0 = sys.argv[1], time.time()
        for seed in range(int(sys.argv[2]), int(sys.argv[3])):
            v = trial(kind, seed)
            if seed < 1000:
                r.setdefault(kind, {})[str(seed)] = v
                json.dump(r, open(RESULTS, 'w'), indent=1, sort_keys=True)
            print(seed, 'ok ' if v['ok'] else 'BAD', {k: (round(x, 3) if isinstance(x, float) else x) for k, x in v.items() if k != 'ok'})
        print('wall %.1f s' % (time.time() - t0))
    e4 = json.load(open(E4.RESULTS))['recipe'] if os.path.exists(E4.RESULTS) else {}
    if e4:
        r['wipe'] = {k: dict(ok=bool(v['covered'] >= 0.8 and len(v.get('ended', [])) > 1 and v['ended'][1] == 'finished'
                                     and v['mug_upright'])) for k, v in e4.items()}
    print()
    for kind in ('grasp', 'place', 'push', 'wipe', 'turn', 'pour'):
        rows = r.get(kind, {})
        if not rows:
            print('%-6s not run' % kind)
            continue
        n, ok = len(rows), sum(1 for v in rows.values() if v['ok'])
        print('%-6s %3d/%-3d  %5.1f %%   threshold %d %%   %s' % (
            kind, ok, n, 100 * ok / n, THRESHOLD[kind], 'PASS' if 100 * ok / n >= THRESHOLD[kind] and n >= 100 else
            ('below' if 100 * ok / n < THRESHOLD[kind] else 'fewer than 100 run')))


if __name__ == '__main__':
    main()
