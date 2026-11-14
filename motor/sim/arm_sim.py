"""Physics for the simulated desk arm, and the wire to the Rust seat.

This file is physics and plumbing only. It builds a MuJoCo scene around
the arm model that `brainos motor self-model desk-arm` generates, works
out what the arm would feel (joint angles, force on the tool, force
between the fingers, slip), writes that to the seat process once per
millisecond and applies the six efforts it gets back. It decides nothing.

Processes:
    this file  <-- s / c lines -->  `brainos motor seat`   (entity, gate, arm loop)
               <-- p, pose, up -->  `brainos motor mind`   (cognition; may be killed)
"""
import math, os, queue, subprocess, threading
import mujoco
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('BRAINOS_BIN', os.path.join(HERE, '..', '..', 'os', 'target', 'release', 'brainos'))
Q0 = [0.0, 90.0, -90.0, -90.0, 0.0, 0.02]
JOINTS = ['pan', 'shoulder', 'elbow', 'wrist', 'roll', 'grip']
G, B, J = mujoco.mjtObj.mjOBJ_GEOM, mujoco.mjtObj.mjOBJ_BODY, mujoco.mjtObj.mjOBJ_JOINT


def arm_xml():
    return subprocess.run([BIN, 'motor', 'self-model', 'desk-arm'], check=True,
                          capture_output=True, text=True).stdout


# ---------------------------------------------------------------- things
def _free(name, pos, yaw, body):
    return (f'<body name="{name}" pos="{pos[0]} {pos[1]} {pos[2]}" euler="0 0 {yaw}">'
            f'<freejoint name="{name}_free"/>{body}</body>')


def block(name, pos, half, mass=0.05, friction=1.0, yaw=0.0, rgba='0.8 0.5 0.2 1'):
    g = (f'<geom name="{name}_g" type="box" size="{half[0]} {half[1]} {half[2]}" mass="{mass}" '
         f'friction="{friction} 0.005 0.0001" contype="1" conaffinity="1" rgba="{rgba}"/>')
    return _free(name, pos, yaw, g)


def puck(name, pos, radius, half_h, mass=0.05, friction=1.0):
    g = (f'<geom name="{name}_g" type="cylinder" size="{radius} {half_h}" mass="{mass}" '
         f'friction="{friction} 0.005 0.0001" contype="1" conaffinity="1" rgba="0.3 0.6 0.8 1"/>')
    return _free(name, pos, 0.0, g)


def pad(name, pos, half=0.03):
    """A marked place on the table. No collision: it is a target, not a thing."""
    return (f'<body name="{name}" pos="{pos[0]} {pos[1]} 0.0005"><geom name="{name}_g" type="box" '
            f'size="{half} {half} 0.0005" contype="0" conaffinity="0" rgba="0.2 0.8 0.3 0.6"/></body>')


BRUSH_GRIP_Z = 0.075     # grip point above the bottom of the head
BRUSH_HANDLE_HALF = 0.042
BRUSH_HEAD_R = 0.009
BRUSH_HEAD_H = 0.016
# The head is modelled as a rigid cylinder, but a brush has bristles. A
# rigid cylinder pressed against a wall touches it along one edge, so
# "touched" would depend on which way the handle happens to lean. A wall
# cell counts as wiped when the head's surface passes within this
# distance of it while the brush is held (worked out from where the head
# is; see mug_task.wiped). Decided on development instances before any
# gate run.
BRISTLE_M = 0.002
# Wet bristles on glazed pottery slide; rubber would drag the mug round.
BRUSH_HEAD_MU = 0.3


def brush(name, pos, yaw=0.0):
    """Stands on its own head. Origin at the bottom centre of the head."""
    fr = 'friction="1.2 0.005 0.0001" contype="1" conaffinity="1"'
    body = (f'<geom name="{name}_head" type="cylinder" size="{BRUSH_HEAD_R} {BRUSH_HEAD_H/2}" '
            f'pos="0 0 {BRUSH_HEAD_H/2}" mass="0.030" contype="1" conaffinity="1" friction="{BRUSH_HEAD_MU} 0.005 0.0001" '
            f'priority="1" rgba="0.9 0.9 0.3 1"/>'
            f'<geom name="{name}_handle" type="box" size="0.006 0.006 0.042" pos="0 0 {BRUSH_HEAD_H+0.042}" '
            f'mass="0.008" {fr} priority="1" rgba="0.5 0.3 0.1 1"/>')
    return _free(name, pos, yaw, body)


MUG_SECTORS, MUG_BANDS, MUG_FLOOR = 16, 2, 0.006


def mug(name, pos, radius, height, yaw=0.0, mass=0.30, friction=1.0, wall=0.004):
    """An open cylinder made of staves, so a tool can go inside it.
    Origin at the bottom centre. `radius` is the inner radius."""
    fr = f'friction="{friction} 0.005 0.0001" contype="1" conaffinity="1"'
    n = MUG_SECTORS
    parts = [f'<geom name="{name}_floor" type="cylinder" size="{radius+wall} {MUG_FLOOR/2}" '
             f'pos="0 0 {MUG_FLOOR/2}" mass="{mass*0.3}" {fr} rgba="0.85 0.85 0.9 1"/>']
    band_h = (height - MUG_FLOOR) / MUG_BANDS
    half_w = (radius + wall) * math.tan(math.pi / n) * 1.03
    m_each = mass * 0.7 / (n * MUG_BANDS)
    for b in range(MUG_BANDS):
        z = MUG_FLOOR + band_h * (b + 0.5)
        for k in range(n):
            a = 360.0 * k / n
            r = radius + wall / 2
            x, y = r * math.cos(math.radians(a)), r * math.sin(math.radians(a))
            parts.append(f'<geom name="{name}_w{b}_{k}" type="box" size="{wall/2} {half_w} {band_h/2}" '
                         f'pos="{x} {y} {z}" euler="0 0 {a}" mass="{m_each}" {fr} rgba="0.85 0.85 0.9 1"/>')
    return _free(name, pos, yaw, ''.join(parts))


# ---------------------------------------------------------------- the sim
def solve(pos, pitch_deg=-90.0, finger_yaw_deg=0.0, grip=0.0):
    """Closed-form inverse kinematics, the same arithmetic as
    `bodyd::deskarm::DeskArmConfig::solve`, for the teleoperated demo and
    the behaviour-cloning baseline, which do not go through the seat."""
    h, l1, l2, tool = 0.11, 0.12, 0.14, 0.03 + 0.03 + 0.6 * 0.06
    x, y, z = pos
    phi = math.radians(pitch_deg)
    r = math.hypot(x, y)
    pan = math.atan2(y, x)
    rw, zw = r - tool * math.cos(phi), z - h - tool * math.sin(phi)
    c = (rw * rw + zw * zw - l1 * l1 - l2 * l2) / (2 * l1 * l2)
    if not -1.0 <= c <= 1.0:
        return None
    a2 = -math.acos(c)
    a1 = math.atan2(zw, rw) - math.atan2(l2 * math.sin(a2), l1 + l2 * math.cos(a2))
    a3 = phi - a1 - a2
    roll = math.degrees(pan) + 90.0 - finger_yaw_deg
    while roll > 90:
        roll -= 180
    while roll < -90:
        roll += 180
    return [math.degrees(pan), math.degrees(a1), math.degrees(a2), math.degrees(a3), roll, grip]


TELEOP_KP = [30.0, 60.0, 40.0, 12.0, 4.0]
TELEOP_KD = [1.2, 2.4, 1.6, 0.35, 0.08]


class ArmSim:
    def __init__(self, things_xml='', places=None, seed=0, style='fluid', record=False, teleop=False):
        xml = arm_xml().replace('</worldbody>', things_xml + '</worldbody>')
        self.m = mujoco.MjModel.from_xml_string(xml)
        self.m.opt.timestep = 0.001
        self.d = mujoco.MjData(self.m)
        self.rng = np.random.default_rng(seed)
        self.places_fn = places or (lambda sim: [])
        self.qadr = [self.m.jnt_qposadr[mujoco.mj_name2id(self.m, J, n)] for n in JOINTS]
        self.vadr = [self.m.jnt_dofadr[mujoco.mj_name2id(self.m, J, n)] for n in JOINTS]
        self.site = mujoco.mj_name2id(self.m, mujoco.mjtObj.mjOBJ_SITE, 'grasp')
        arm_bodies = {mujoco.mj_name2id(self.m, B, n) for n in
                      ['pedestal', 'turret', 'upper', 'fore', 'wristlink', 'hand', 'finger_l', 'finger_r']}
        self.arm_bodies = arm_bodies
        self.arm_geom = np.array([self.m.geom_bodyid[g] in arm_bodies for g in range(self.m.ngeom)])
        fl, fr = (mujoco.mj_name2id(self.m, B, n) for n in ('finger_l', 'finger_r'))
        self.finger_of = np.array([1 if self.m.geom_bodyid[g] == fl else 2 if self.m.geom_bodyid[g] == fr else 0
                                   for g in range(self.m.ngeom)])
        self.ground = mujoco.mj_name2id(self.m, G, 'ground')
        # only the working end of a tool wipes: a handle leaning on the
        # rim has cleaned nothing
        self.head_geom = np.array([(mujoco.mj_id2name(self.m, G, g) or '').endswith('_head')
                                   for g in range(self.m.ngeom)])
        for a, v in zip(self.qadr[:5], Q0[:5]):
            self.d.qpos[a] = math.radians(v)
        self.d.qpos[self.qadr[5]] = Q0[5]
        self.d.qpos[self.m.jnt_qposadr[mujoco.mj_name2id(self.m, J, 'grip_r')]] = Q0[5]
        mujoco.mj_forward(self.m, self.d)
        self.t_us = 0
        self.teleop = teleop
        self.seat = None if teleop else subprocess.Popen([BIN, 'motor', 'seat'] + (['--stop-go'] if style == 'stop-go' else []),
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.mind = None
        self.mind_q = queue.Queue()
        self.ups = []            # every `up` line the seat sent
        self.held = -1
        self.last = dict(tool_n=0.0, grip_n=0.0, slip=0.0)
        self.drive = None
        self.push = None         # (body id, force xyz, until_us): an outside shove
        self.probe = None        # called every tick with the sim: a task's own bookkeeping
        self.record = record
        self.log = []            # per tick, when recording
        self.touched = set()     # (geom name) pairs touched by anything held
        self.peak_tool_n = 0.0
        self._percepts = []      # (due_us, line)
        self.percept_every_us, self.percept_lag_us = 200_000, 150_000

    # -- what the arm feels ------------------------------------------------
    def body(self, name):
        return mujoco.mj_name2id(self.m, B, name)

    def pos(self, name):
        return self.d.xpos[self.body(name)].copy()

    def yaw(self, name):
        R = self.d.xmat[self.body(name)].reshape(3, 3)
        return math.degrees(math.atan2(R[1, 0], R[0, 0]))

    def upright(self, name, within_deg=20.0):
        R = self.d.xmat[self.body(name)].reshape(3, 3)
        return math.degrees(math.acos(max(-1.0, min(1.0, R[2, 2])))) < within_deg

    def tool_pos(self):
        return self.d.site_xpos[self.site].copy()

    def _point_vel(self, body, point):
        jac = np.zeros((3, self.m.nv))
        mujoco.mj_jac(self.m, self.d, jac, None, point, body)
        return jac @ self.d.qvel

    def sense(self):
        m, d = self.m, self.d
        f6 = np.zeros(6)
        per_finger = {1: {}, 2: {}}       # finger -> body -> normal force
        contacts = []
        for i in range(d.ncon):
            c = d.contact[i]
            mujoco.mj_contactForce(m, d, i, f6)
            if f6[0] <= 0:
                continue
            contacts.append((c.geom1, c.geom2, f6[0], c.pos.copy(), c.frame[:3].copy()))
            for a, b in ((c.geom1, c.geom2), (c.geom2, c.geom1)):
                fin = self.finger_of[a]
                # the table is not a thing that can be held
                if fin and not self.arm_geom[b] and m.geom_bodyid[b] != 0:
                    body = m.geom_bodyid[b]
                    per_finger[fin][body] = per_finger[fin].get(body, 0.0) + f6[0]
        both = set(per_finger[1]) & set(per_finger[2])
        self.held = max(both, key=lambda b: min(per_finger[1][b], per_finger[2][b])) if both else -1
        grip_n = min(per_finger[1][self.held], per_finger[2][self.held]) if self.held >= 0 else 0.0
        tool_n, slip = 0.0, 0.0
        for g1, g2, fn, p, n in contacts:
            b1, b2 = m.geom_bodyid[g1], m.geom_bodyid[g2]
            mine1 = self.arm_geom[g1] or b1 == self.held
            mine2 = self.arm_geom[g2] or b2 == self.held
            if mine1 != mine2:
                tool_n += fn
                for a, b in ((g1, g2), (g2, g1)):
                    if m.geom_bodyid[a] == self.held and self.head_geom[a]:
                        self.touched.add(mujoco.mj_id2name(m, G, b))
            elif self.held >= 0 and ((self.finger_of[g1] and b2 == self.held) or (self.finger_of[g2] and b1 == self.held)):
                v = self._point_vel(b1, p) - self._point_vel(b2, p)
                v -= (v @ n) * n
                slip = max(slip, float(np.linalg.norm(v)))
        # a shove from outside is something touching the arm, and is felt
        # as such (a real arm feels it as torque its own motion does not
        # explain)
        if self.push and self.t_us < self.push[2] and (
                self.push[0] in self.arm_bodies or self.push[0] == self.held):
            tool_n += float(np.linalg.norm(self.push[1]))
        self.last = dict(tool_n=tool_n, grip_n=grip_n, slip=slip)
        if self.probe:
            self.probe(self)
        self.peak_tool_n = max(self.peak_tool_n, tool_n)
        q = [math.degrees(d.qpos[a]) for a in self.qadr[:5]] + [d.qpos[self.qadr[5]]]
        dq = [math.degrees(d.qvel[a]) for a in self.vadr[:5]] + [d.qvel[self.vadr[5]]]
        return q, dq, tool_n, grip_n, slip

    # -- the wire ----------------------------------------------------------
    def _seat_read(self):
        while True:
            line = self.seat.stdout.readline()
            if not line:
                raise RuntimeError('the seat process ended')
            if line.startswith('c '):
                return line.split()
            self.ups.append(line.strip())
            if self.mind and self.mind.poll() is None:
                try:
                    self.mind.stdin.write(line)
                    self.mind.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass

    def ask(self, ident, verb, **args):
        """A request to the seat, as cognition would send it."""
        self.seat.stdin.write(f'ask {ident} {verb} ' + ' '.join(f'{k}={v}' for k, v in args.items()) + '\n')

    def start_mind(self, recipe_path, stop_go=False):
        self.mind = subprocess.Popen([BIN, 'motor', 'mind', recipe_path] + (['--stop-go'] if stop_go else []),
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

        def pump(p, q):
            for line in p.stdout:
                q.put(line.strip())
        threading.Thread(target=pump, args=(self.mind, self.mind_q), daemon=True).start()

    def kill_mind(self):
        """A real SIGKILL. Returns once the process is gone."""
        self.mind.kill()
        self.mind.wait()

    def _to_mind(self, line):
        if self.mind and self.mind.poll() is None:
            try:
                self.mind.stdin.write(line + '\n')
                self.mind.stdin.flush()
            except (BrokenPipeError, OSError):
                pass

    def step(self):
        """One millisecond."""
        q, dq, tool_n, grip_n, slip = self.sense()
        self.seat.stdin.write('s %d %s %s %.5f %.5f %.6f\n' % (
            self.t_us, ' '.join('%.6f' % v for v in q), ' '.join('%.6f' % v for v in dq),
            tool_n, grip_n, slip))
        self.seat.stdin.flush()
        c = self._seat_read()
        e = [float(x) for x in c[1:7]]
        self.drive = dict(running=c[7] == '1', goal=int(c[8]), stiff=float(c[9]), squeeze=float(c[10]),
                          slip=c[11] == '1', yielded=c[12] == '1', wait=c[13] == '1', standing=c[14] == '1')
        self.d.ctrl[:6] = e
        self.d.xfrc_applied[:] = 0
        if self.push and self.t_us < self.push[2]:
            self.d.xfrc_applied[self.push[0], :3] = self.push[1]
        mujoco.mj_step(self.m, self.d)
        self.t_us += 1000
        # perception: slow, and late
        if self.mind:
            if self.t_us % self.percept_every_us == 0:
                for name, pos, yaw, attrs in self.places_fn(self):
                    line = 'p %d %s %.5f %.5f %.5f %.3f %s' % (
                        self.t_us, name, pos[0], pos[1], pos[2], yaw,
                        ' '.join(f'{k}={v:.5f}' for k, v in attrs.items()))
                    self._percepts.append((self.t_us + self.percept_lag_us, line))
            while self._percepts and self._percepts[0][0] <= self.t_us:
                self._to_mind(self._percepts.pop(0)[1])
            if self.t_us % 50_000 == 0:
                p = self.tool_pos()
                self._to_mind('pose %d %.5f %.5f %.5f' % (self.t_us, p[0], p[1], p[2]))
            while True:
                try:
                    line = self.mind_q.get_nowait()
                except queue.Empty:
                    break
                if line.startswith('ask '):
                    self.seat.stdin.write(line + '\n')
                else:
                    self.ups.append('mind ' + line)
        if self.record:
            self.log.append((self.t_us, *self.tool_pos(), self.d.qpos[self.qadr[5]],
                             math.degrees(self.d.qpos[self.qadr[4]]), grip_n, tool_n, slip,
                             self.drive['stiff'], int(self.drive['running']), self.drive['goal'],
                             int(self.drive['slip']), int(self.drive['yielded']), int(self.drive['wait']),
                             self.drive['squeeze']))
        return self.drive

    def step_teleop(self, q_des, squeeze=8.0):
        """One millisecond with the joints driven straight from here: a
        plain PD loop to `q_des` with the simulator's own gravity term.
        No seat, no gate, no primitives. Used only to make a
        demonstration and to run the behaviour-cloning baseline."""
        q, dq, tool_n, grip_n, slip = self.sense()
        for i in range(5):
            dof = self.vadr[i]
            u = (TELEOP_KP[i] * math.radians(q_des[i] - q[i]) - TELEOP_KD[i] * math.radians(dq[i])
                 + self.d.qfrc_bias[dof])
            self.d.ctrl[i] = max(-2.9, min(2.9, u))
        f = 2500.0 * (q_des[5] - q[5]) - 25.0 * dq[5]
        self.d.ctrl[5] = max(-squeeze, min(squeeze, f))
        mujoco.mj_step(self.m, self.d)
        self.t_us += 1000
        return q, tool_n, grip_n

    def run(self, ms):
        for _ in range(ms):
            self.step()

    def run_until(self, pred, max_ms):
        for _ in range(max_ms):
            self.step()
            if pred():
                return True
        return False

    def ended(self, ident):
        """The seat's `ended` report for a run, if it has come."""
        for u in self.ups:
            f = u.split()
            if len(f) >= 4 and f[0] == 'up' and f[1] == str(ident) and f[2] == 'ended':
                return f[3]
        return None

    def answered(self, ident):
        for u in self.ups:
            f = u.split()
            if len(f) >= 3 and f[0] == 'up' and f[1] == str(ident):
                return ' '.join(f[2:])
        return None

    def mind_done(self):
        for u in self.ups:
            if u.startswith('mind done'):
                return u[5:]
        return None

    def stat(self):
        self.seat.stdin.write('stat\n')
        self.step()
        for u in reversed(self.ups):
            if u.startswith('up stat'):
                return dict(kv.split('=') for kv in u.split()[2:])
        return {}

    def close(self):
        for p in (self.mind, self.seat):
            if p and p.poll() is None:
                try:
                    p.stdin.write('quit\n')
                    p.stdin.flush()
                except (BrokenPipeError, OSError, ValueError):
                    pass
                p.kill()
                p.wait()


WHOLE = dict(x_min=-0.35, x_max=0.35, y_min=-0.35, y_max=0.35, z_min=-0.009, z_max=0.46)


def hold(sim):
    """The standing grant every run starts with."""
    sim.step()
    sim.ask('h', 'arm.hold', duration_ms=600000, force_n=20, rate_dps=270, **WHOLE)
