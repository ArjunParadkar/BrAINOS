#!/usr/bin/env python3
"""
nerves.py -- BLUR's first body: the v36 MuJoCo twin behind the real nerve formats.

The Teensy (Domain 2) sees exactly what it would see on the robot: CAN
frames from twelve B-G431B-ESC1s and three ESP32-S3 nodes on three buses,
a BMI270 SPI burst, the e-stop line, the pack voltage. This process plays
everything below that line -- the Silicon layer:

  * each node's impedance loop (q*, kp, kd, tau_ff), run at the physics
    rate (2 kHz), with its own current clamp, a probe-current cap until
    calibration is committed, and the torque/speed derate from the
    descriptor. The node holds the PHYSICAL envelope by itself, so it holds
    if the Teensy dies or misbehaves: once committed, its current ceiling
    and joint limits can only be narrowed and its calibration cannot be
    rewritten (both are open again only while uncommitted or e-stopped,
    when it delivers no torque); setpoint gains are capped at the
    envelope's kp_max; a setpoint older than the watchdog becomes pure
    damping (a slow sag, not a drop);
  * MT6701 encoders: 14-bit quantisation, an assembly offset per joint
    (where the magnet was glued), turns counted since power-on, and for
    the 4015s the encoder on the motor side of the 16:1 gearbox;
  * CAN timing: every frame occupies its bus for 135 us (worst-case
    stuffing at 1 Mbit/s) plus jitter, so a flooded bus is late, as it
    would be;
  * the BMI270: mounting tilt, gyro bias, noise, LSB quantisation;
  * the latching e-stop: cuts the motor rail; no frame closes it.

The MJCF's own position-servo actuators are replaced by plain torque
motors (gain 1, no bias) so the only controller in the loop is the
node's; the MJCF forcerange stays as the hard torque limit.

Lockstep: one 'D' datagram from the Autonomic = one millisecond; the
reply 'U' carries what the pins read at the end of it. 'C' datagrams are
the test harness's hand (faults, jig, pushes), not a nerve.

  python blur/sim/nerves.py --sock /tmp/blur_nerves.sock [--jig] [--fault flip:RR_knee] ...
"""
import argparse, json, math, os, socket, struct, sys, time

import mujoco
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import wire  # noqa: E402

sys.path.insert(0, os.path.dirname(HERE))
import blurcfg  # noqa: E402  (per-machine MJCF location, sha-checked)
DESC = blurcfg.load_descriptor()
TWO_PI = 2 * math.pi
ENC_COUNTS = 1 << DESC['enc_bits']
PROBE_CAP_FRAC = 0.9          # max fraction of driver peak before Commit (homing the spine needs most of it)
PROBE_WATCHDOG_MS = 50
LIMIT_K = 40.0               # N*m/rad, the node's joint-limit spring (scaled per motor below)
SP_WATCHDOG_MS = 60
DAMP_KD_PER_TAU = 0.24        # after the watchdog: pure damping, kd = this x deliverable torque (= the stand kd)
FB_PERIOD_MS = 5              # 200 Hz, the bus budget's rate
RAW_PERIOD_MS = 100
BUS_FRAME_S = wire.BITS_WORST_8 / 1e6
DEBUG = bool(os.environ.get('BLUR_TWIN_DEBUG'))   # BLUR_TWIN_DEBUG=1: tail + joint trace to stderr every 0.5 s


class Node:
    """One actuator channel as its node firmware would run it."""

    def __init__(self, a, m, rng, faults):
        self.a = a
        self.name = a['name']
        self.motor = a['motor']
        self.tendon = a['tendon']
        self.kt = a['kt_out'] if a['kt_out'] == a['kt_out'] else None
        self.peak = a['drv_peak_a']
        self.tau_max = a['tau_max']
        self.w0 = a['w0']
        self.act = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_ACTUATOR, a['name'])
        if self.tendon:
            self.ten = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_TENDON, a['target'])
        else:
            j = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_JOINT, a['target'])
            self.qadr, self.dadr = m.jnt_qposadr[j], m.jnt_dofadr[j]
        g = a['gear'] if a['encoder'] == 'motor' else 1.0
        self.enc_div = g                       # output units per encoder turn = 2pi/g (rad) or drum circumference (m)
        self.step = (TWO_PI / ENC_COUNTS / g) if not self.tendon else (0.005 * TWO_PI / ENC_COUNTS)
        # assembly truth
        self.off_true = rng.uniform(0, TWO_PI)                  # magnet angle (encoder radians)
        self.enc_sign = -1.0 if f'mirror:{self.name}' in faults else 1.0
        self.torque_sign = -1.0 if f'flip:{self.name}' in faults else 1.0
        self.dead = f'dead_enc:{self.name}' in faults
        self.reset_config()
        self.c_boot = None
        self.q_prev = None
        self.dq = 0.0
        self.frozen = None
        self.i = 0.0
        self.clamped = False
        self.tau_requested = 0.0
        self.tau_limit = 0.0
        deliv = self.tau_max if self.kt is None else min(self.tau_max, self.kt * self.peak)
        self.kp_max = 3000.0 if self.tendon else 12.0 * deliv          # supervisor.rs Envelope::physical
        self.kd_damp = 5.0 if self.tendon else DAMP_KD_PER_TAU * deliv
        self.refused = 0                                                # config writes the node would not take
        self.ceil = None                                                # (cur, qmin, qmax) fixed at commit

    def reset_config(self):
        self.dir = 1.0
        self.offset = 0.0
        self.turn_offset = 0.0
        self.qmin, self.qmax = -1e9, 1e9
        self.cur_limit = PROBE_CAP_FRAC * self.peak
        self.committed = False
        self.estopped = False
        self.probe_tau, self.probe_t = 0.0, -1e9
        self.sp, self.sp_t = None, -1e9

    def q_phys(self, d):
        return d.ten_length[self.ten] if self.tendon else d.qpos[self.qadr]

    def enc_cont(self, d):
        """continuous encoder-frame position (output units) as the node tracks it since power-on"""
        qp = self.enc_sign * self.q_phys(d)
        unit = (0.005 * TWO_PI) if self.tendon else (TWO_PI / self.enc_div)
        x = qp / unit * TWO_PI + self.off_true                  # encoder shaft angle, rad
        if self.c_boot is None:                                 # power-on: the node centres its turn counter,
            self.c_boot = -TWO_PI * round(x / TWO_PI)           # so the first reading is in [-pi, pi)
        x = x + self.c_boot
        counts = math.floor(x / TWO_PI * ENC_COUNTS)
        if self.dead:
            if self.frozen is None:
                self.frozen = counts
            counts = self.frozen
        return counts * self.step, counts

    def q_node(self, d):
        e, _ = self.enc_cont(d)
        return self.dir * e + self.offset + self.turn_offset * (TWO_PI / self.enc_div)

    def status(self, rail):
        s = 0
        if self.sp is not None and self.sp['flags'] & wire.SP_ENABLE and rail and not self.estopped:
            s |= wire.ST_ENABLED
        if not self.dead and self.a['encoder'] != 'none':
            s |= wire.ST_ENC_OK
        if self.clamped: s |= wire.ST_OVERCURRENT
        if self.estopped: s |= wire.ST_ESTOPPED
        if self.committed: s |= wire.ST_CALIBRATED
        return s

    def config(self, key, value):
        # committed and live: the physical envelope is the node's own. Limits
        # may only narrow, calibration may not change (it would move where
        # the limits are); both reopen only uncommitted or e-stopped.
        locked = self.committed and not self.estopped and self.ceil is not None
        if locked and key in (1, 2, 6):
            self.refused += 1
            return float('nan')                 # acknowledged as refused
        if key == 1: self.offset = value
        elif key == 2: self.dir = 1.0 if value >= 0 else -1.0
        elif key == 3:
            v = max(0.0, min(value, self.peak))
            if locked and v > self.ceil[0]:
                self.refused += 1; v = self.ceil[0]
            self.cur_limit = v
        elif key == 4:
            if locked and value < self.ceil[1]:
                self.refused += 1; value = self.ceil[1]
            self.qmin = value
        elif key == 5:
            if locked and value > self.ceil[2]:
                self.refused += 1; value = self.ceil[2]
            self.qmax = value
        elif key == 6: self.turn_offset = round(value)
        elif key == 7:
            self.committed = value >= 0.5
            if self.committed:
                self.estopped = False             # re-acquisition is the only way back from an e-stop
                self.ceil = (self.cur_limit, self.qmin, self.qmax)
            else:
                self.ceil = None
        elif key == 8: self.probe_tau = value
        return value

    def torque(self, d, t_ms, rail, dt):
        """phase current and physical joint torque for one physics step"""
        qn = self.q_node(d)
        if self.q_prev is None: self.q_prev = qn
        self.dq += 0.25 * ((qn - self.q_prev) / dt - self.dq)
        self.q_prev = qn
        tau = 0.0
        if rail and not self.estopped:
            if t_ms - self.probe_t <= PROBE_WATCHDOG_MS and self.probe_tau != 0.0:
                tau = self.probe_tau
            elif self.sp is not None and (self.sp['flags'] & wire.SP_ENABLE) and t_ms - self.sp_t <= SP_WATCHDOG_MS:
                sp = self.sp
                kp = min(max(sp['kp'], 0.0), self.kp_max); kd = min(max(sp['kd'], 0.0), 0.2 * self.kp_max)
                q = sp['q'] if not self.committed else min(max(sp['q'], self.qmin), self.qmax)
                tau = kp * (q - qn) - kd * self.dq + sp['tau']
            elif self.committed and self.sp is not None and (self.sp['flags'] & wire.SP_ENABLE):
                # the Teensy went quiet: hold nothing, resist motion -- a slow sag
                tau = -self.kd_damp * self.dq
        if rail and not self.estopped and self.committed and (self.sp is not None and self.sp['flags'] & wire.SP_ENABLE):
            # the node's own joint-limit guard: outside its pushed limits it
            # pushes back with a stiff spring, whatever the setpoint says
            if qn < self.qmin:
                tau = max(tau, LIMIT_K * (self.qmin - qn) - 0.1 * LIMIT_K * min(0.0, self.dq) * 0.02)
            elif qn > self.qmax:
                tau = min(tau, -LIMIT_K * (qn - self.qmax) - 0.1 * LIMIT_K * max(0.0, self.dq) * 0.02)
        cap = self.cur_limit if self.committed else min(self.cur_limit, PROBE_CAP_FRAC * self.peak)
        self.tau_requested = tau
        if self.kt is not None:
            i = tau / self.kt
            self.clamped = abs(i) > cap
            i = max(-cap, min(cap, i))
            tau = i * self.kt
            self.tau_limit = cap * self.kt
        else:
            i = tau / max(self.tau_max, 1e-9) * self.peak
            self.clamped = False
            self.tau_limit = self.tau_max
        # torque available at this speed (descriptor's derate)
        w = abs(self.dq)
        avail = self.tau_max if w <= 0.5 * self.w0 else (0.0 if w >= self.w0 else self.tau_max * (self.w0 - w) / (0.5 * self.w0))
        tau = max(-avail, min(avail, tau))
        self.i = i
        tau_motor = self.dir * tau
        return self.enc_sign * self.torque_sign * tau_motor


class Twin:
    def __init__(self, args):
        self.args = args
        self.rng = np.random.default_rng(args.seed)
        faults = set(args.fault or [])
        self.faults = faults
        scene = os.path.join(DESC['mjcf_dir'], f'scene_{args.scene}.xml')
        self.m = mujoco.MjModel.from_xml_path(scene)
        m = self.m
        for i in range(m.nu):                                  # the node is the controller now
            m.actuator_gainprm[i, :] = 0; m.actuator_gainprm[i, 0] = 1.0
            m.actuator_biasprm[i, :] = 0
        self.d = mujoco.MjData(m)
        key = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_KEY, 'stand')
        mujoco.mj_resetDataKeyframe(m, self.d, key)
        self.jig = args.jig
        self.jig_pose = self.d.qpos[:7].copy(); self.jig_pose[2] += args.jig_height
        if self.jig:
            self.d.qpos[:7] = self.jig_pose
        mujoco.mj_forward(m, self.d)
        self.nodes = [Node(a, m, self.rng, faults) for a in DESC['actuators']]
        for n in self.nodes: n.enc_cont(self.d)                  # power-on: turn counters start here
        self.by_slot = {(a['node'], a['slot']): n for a, n in zip(DESC['actuators'], self.nodes)}
        self.node_bus = {nd['id']: 'ABC'.index(nd['bus']) for nd in DESC['nodes']}
        self.node_kind = {nd['id']: (wire.KIND_ESC1 if nd['kind'] == 'esc1' else wire.KIND_ESP32S3) for nd in DESC['nodes']}
        self.node_slots = {nd['id']: len(nd['slots']) for nd in DESC['nodes']}
        # silicon identity: 96-bit UIDs, seeded so the twin is the same unit every boot
        urng = np.random.default_rng(args.unit)
        self.uid = {nid: bytes(urng.integers(0, 256, 12, dtype=np.uint8)) for nid in self.node_bus}
        self.absent = set()
        for f in faults:
            if f.startswith('phantom:'):
                name = f.split(':', 1)[1]
                a = next(a for a in DESC['actuators'] if a['name'] == name)
                self.absent.add(a['node'])
        self.forged = {int(f.split(':')[1], 0) for f in faults if f.startswith('forged:')}
        self.second = {int(f.split(':')[1], 0) for f in faults if f.startswith('forged2:')}
        self.strangers = {int(f.split(':')[1], 0) for f in faults if f.startswith('stranger:')}
        self.imu_off = 'imu_off' in faults
        self.imu_drop_ms = next((int(f.split(':')[1]) for f in faults if f.startswith('imu_drop:')), None)
        # IMU assembly truth
        r, p = self.rng.normal(0, math.radians(1.0), 2)
        self.imu_R = rot_rp(r, p)
        self.gyro_bias = self.rng.normal(0, 0.008, 3)
        self.imu_site = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SITE, 'imu')
        self.sacc = m.sensor_adr[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SENSOR, 'imu_acc')]
        self.sgyr = m.sensor_adr[mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_SENSOR, 'imu_gyro')]
        self.estop_latched = False
        self.relay_open = False
        self.vbat = 24.0
        self.t_ms = 0
        self.substeps = max(1, round(0.001 / m.opt.timestep))
        self.dt = m.opt.timestep
        self.bus_free = [0.0, 0.0, 0.0]
        self.inflight = []                                      # (deliver_time_s, seq, direction, frame)
        self.seq = 0
        self.lat = tuple(args.latency)
        self.stats = dict(frames_down=0, frames_up=0, max_bus_load=[0.0, 0.0, 0.0])
        self.bus_bits_ms = [0, 0, 0]
        self.push_force = None
        self.clamp = dict(worst_ratio=0.0, requests_over=0, torque_with_rail_off=0, torque_while_estopped=0)
        self.recorder = None
        if args.record:
            self.recorder = Recorder(self, args.record)

    # -- CAN timing ------------------------------------------------------
    def put_on_bus(self, f, direction):
        now = self.t_ms * 1e-3
        b = f.bus
        start = max(now, self.bus_free[b])
        done = start + BUS_FRAME_S
        self.bus_free[b] = done
        self.bus_bits_ms[b] += wire.BITS_WORST_8
        jitter = self.rng.uniform(*self.lat) * 1e-3
        self.seq += 1
        self.inflight.append((done + jitter, self.seq, direction, f))

    def due(self, direction, until):
        out, keep = [], []
        for item in self.inflight:
            (out if item[2] == direction and item[0] <= until else keep).append(item)
        self.inflight = keep
        out.sort(key=lambda x: (x[0], x[1]))
        return [x[3] for x in out]

    # -- node firmware: frames in ------------------------------------------
    def node_rx(self, f):
        if wire.is_estop(f):
            for n in self.nodes:
                if self.node_bus.get(n.a['node']) == f.bus:
                    n.estopped = True
            return
        nid = f.node
        if f.fc == wire.FC_CENSUS:
            for node, bus in self.node_bus.items():
                if bus != f.bus or node in self.absent: continue
                uid = self.uid[node]
                if node in self.forged:
                    uid = bytes(b ^ 0x5A for b in uid)
                for p in wire.identity_pages(bus, node, self.node_kind[node], self.node_slots[node], 1, uid):
                    self.put_on_bus(p, 'up')
                if node in self.second:
                    other = bytes(b ^ 0xA5 for b in self.uid[node])
                    for p in wire.identity_pages(bus, node, self.node_kind[node], self.node_slots[node], 1, other):
                        self.put_on_bus(p, 'up')
            for s in self.strangers:
                if f.bus == 1:
                    for p in wire.identity_pages(1, s, wire.KIND_ESP32S3, 1, 1, bytes(range(12))):
                        self.put_on_bus(p, 'up')
            return
        if nid in self.absent or self.node_bus.get(nid) != f.bus:
            return
        if f.fc == wire.FC_SETPOINT:
            n = self.by_slot.get((nid, f.data[0]))
            if n is None: return
            sp = wire.setpoint_decode(f, n.motor)
            if sp is not None:
                n.sp, n.sp_t = sp, self.t_ms
        elif f.fc == wire.FC_CONFIG:
            c = wire.config_decode(f)
            n = self.by_slot.get((nid, c['slot'])) if c else None
            if n is None: return
            v = n.config(c['key'], c['value'])
            if c['key'] == 8:
                n.probe_t = self.t_ms
                return                                          # probe torque is not acknowledged; it is a stream
            self.put_on_bus(wire.config_ack(f.bus, nid, c['slot'], c['key'], v, c['seq']), 'up')

    # -- node firmware: frames out -----------------------------------------
    def node_tx(self):
        rail = self.rail_on()
        for (nid, slot), n in self.by_slot.items():
            if nid in self.absent: continue
            bus = self.node_bus[nid]
            phase = (nid * 7 + slot) % FB_PERIOD_MS
            if self.t_ms % FB_PERIOD_MS == phase:
                q = n.q_node(self.d)
                if n.a['encoder'] == 'none':
                    q += self.rng.normal(0, 0.01)
                self.put_on_bus(wire.feedback_encode(bus, nid, n.motor, slot, n.status(rail), q, n.dq, n.i), 'up')
            if self.t_ms % RAW_PERIOD_MS == (nid + slot) % RAW_PERIOD_MS:
                _, counts = n.enc_cont(self.d)
                self.put_on_bus(wire.raw_encode(bus, nid, slot, n.status(rail), counts % ENC_COUNTS,
                                                counts // ENC_COUNTS, 35), 'up')

    def rail_on(self):
        return not self.estop_latched and not self.relay_open

    # -- IMU --------------------------------------------------------------
    def imu(self):
        if self.imu_off or (self.imu_drop_ms is not None and self.t_ms >= self.imu_drop_ms):
            return None
        if self.jig:
            # the jig holds the chest kinematically (qpos reset, qvel zeroed every
            # step), which MuJoCo's accelerometer sees as free fall; a rigid jig's
            # chest is still, so the specific force is exactly R^T g and the rate 0
            R = self.d.site_xmat[self.imu_site].reshape(3, 3)
            acc_true, gyr_true = R.T @ np.array([0.0, 0.0, wire.G]), np.zeros(3)
        else:
            acc_true = self.d.sensordata[self.sacc:self.sacc + 3]
            gyr_true = self.d.sensordata[self.sgyr:self.sgyr + 3]
        acc = self.imu_R @ acc_true + self.rng.normal(0, 0.03, 3)
        gyr = self.imu_R @ gyr_true + self.gyro_bias + self.rng.normal(0, 0.003, 3)
        a = [int(np.clip(round(x / wire.G * wire.ACC_LSB_PER_G), -32768, 32767)) for x in acc]
        g = [int(np.clip(round(math.degrees(x) * wire.GYR_LSB_PER_DPS), -32768, 32767)) for x in gyr]
        return wire.bmi270_pack(a, g, int(self.t_ms * 1e-3 / 39.0625e-6))

    # -- one millisecond --------------------------------------------------
    def tick(self, relay_open, frames_down):
        self.relay_open = relay_open
        for f in frames_down:
            self.put_on_bus(f, 'down')
            self.stats['frames_down'] += 1
        t_end = (self.t_ms + 1) * 1e-3
        for k in range(self.substeps):
            t_sub = self.t_ms * 1e-3 + (k + 1) * self.dt
            for f in self.due('down', t_sub):
                self.node_rx(f)
            rail = self.rail_on()
            for n in self.nodes:
                self.d.ctrl[n.act] = n.torque(self.d, self.t_ms, rail, self.dt)
                applied = abs(float(self.d.ctrl[n.act]))
                if n.committed and n.kt is not None and n.tau_limit > 0:
                    over = applied / n.tau_limit
                    if over > self.clamp['worst_ratio']:
                        self.clamp['worst_ratio'] = over
                    if abs(n.tau_requested) > n.tau_limit * 1.0001:
                        self.clamp['requests_over'] += 1
                if not rail and applied > 1e-9:
                    self.clamp['torque_with_rail_off'] += 1
                if n.estopped and applied > 1e-9:
                    self.clamp['torque_while_estopped'] += 1
            if self.push_force is not None:
                body, force, until = self.push_force
                if self.t_ms < until:
                    self.d.xfrc_applied[body, :3] = force
                else:
                    self.d.xfrc_applied[body, :] = 0; self.push_force = None
            if self.jig:
                self.d.qpos[:7] = self.jig_pose; self.d.qvel[:6] = 0
            mujoco.mj_step(self.m, self.d)
        self.node_tx()
        if DEBUG and self.t_ms % 500 == 0:
            tp = self.nodes[19]
            sp = tp.sp
            print(f'[twin t={self.t_ms}] L_pitch={tp.q_phys(self.d):+.4f} qnode={tp.q_node(self.d):+.4f} '
                  f's1={self.nodes[16].q_phys(self.d):+.3f} s2={self.nodes[18].q_phys(self.d):+.3f} '
                  f'winch sp={sp and (sp["flags"], round(sp["kd"],1))} committed={tp.committed} off={tp.offset:+.4f}',
                  file=sys.stderr, flush=True)
            if self.nodes[19].committed:
                st = {a['name']: a['q_stand'] for a in DESC['actuators']}
                tj = [mujoco.mj_name2id(self.m, mujoco.mjtObj.mjOBJ_JOINT, f'tail{k}_pitch') for k in range(9)]
                print('   tail:', [round(float(self.d.qpos[self.m.jnt_qposadr[j]]), 3) for j in tj],
                      'winch force', round(float(self.d.actuator_force[self.nodes[19].act]), 3),
                      'ctrl', round(float(self.d.ctrl[self.nodes[19].act]), 3), 'qfrc_actuator tail0',
                      round(float(self.d.qfrc_actuator[self.m.jnt_dofadr[tj[0]]]), 4), file=sys.stderr, flush=True)
                print('   off-stand:', ' '.join(f'{n.name}={n.q_phys(self.d) - st[n.name]:+.3f}' for n in self.nodes if not n.tendon),
                      file=sys.stderr, flush=True)
        self.t_ms += 1
        for b in range(3):
            self.stats['max_bus_load'][b] = max(self.stats['max_bus_load'][b], self.bus_bits_ms[b] / 1000.0)
            self.bus_bits_ms[b] = 0
        up = self.due('up', t_end)
        self.stats['frames_up'] += len(up)
        if self.recorder: self.recorder.maybe_frame()
        return up

    def rollout(self, msg):
        return _rollout(self, msg)

    def control(self, msg):
        c = msg.get('cmd')
        if c == 'estop':
            self.estop_latched = bool(msg.get('pressed', True))
        elif c == 'jig':
            self.jig = bool(msg.get('on', True))
            if self.jig: self.jig_pose = self.d.qpos[:7].copy()
        elif c == 'drop':                                   # take the body out of the jig onto the floor
            self.jig = False
        elif c == 'push':
            b = mujoco.mj_name2id(self.m, mujoco.mjtObj.mjOBJ_BODY, msg.get('body', 'chest'))
            self.push_force = (b, np.array(msg['force'], float), self.t_ms + int(msg.get('ms', 100)))
        elif c == 'state':
            pass
        elif c == 'fault':
            self.faults.add(msg['fault'])
        if c == 'clamp_reset':
            self.clamp = dict(worst_ratio=0.0, requests_over=0, torque_with_rail_off=0, torque_while_estopped=0)
        if c == 'release':
            # lower the jig to the given chest height, then let go
            self.jig_pose[2] = float(msg.get('height', 0.272))
            self.d.qpos[:7] = self.jig_pose; self.d.qvel[:6] = 0
            mujoco.mj_forward(self.m, self.d)
            self.jig = False
        if c == 'rollout':
            return self.rollout(msg)
        if c == 'reseat':
            # the harness's hand: stand the body back on its feet (logged by the caller)
            key = mujoco.mj_name2id(self.m, mujoco.mjtObj.mjOBJ_KEY, 'stand')
            self.d.qpos[:] = self.m.key_qpos[key]; self.d.qvel[:] = 0; self.d.xfrc_applied[:] = 0
            self.push_force = None
            mujoco.mj_forward(self.m, self.d)
        up_z = float(self.d.xmat[mujoco.mj_name2id(self.m, mujoco.mjtObj.mjOBJ_BODY, 'chest')][8])
        return dict(t_ms=self.t_ms, chest=self.d.qpos[:7].tolist(), rail=self.rail_on(), up=up_z, clamp=self.clamp,
                    node_refused=sum(n.refused for n in self.nodes),
                    estopped=sum(1 for n in self.nodes if n.estopped),
                    estop=self.estop_latched, stats=self.stats,
                    q={n.name: float(n.q_phys(self.d)) for n in self.nodes},
                    tau={n.name: float(self.d.actuator_force[n.act]) for n in self.nodes},
                    current={n.name: float(n.i) for n in self.nodes})


def _rollout(twin, msg):
    m, d0 = twin.m, twin.d
    d = mujoco.MjData(m)
    mujoco.mj_copyData(d, m, d0)
    jig = twin.jig
    pose = {a['name']: (a['q_rest_stand'] if a['tendon'] else a['q_stand']) for a in DESC['actuators']}
    joint, angle = msg.get('joint'), msg.get('angle')
    ms = int(msg.get('ms', 1500))
    start = {n.name: float(n.q_phys(d)) for n in twin.nodes}
    chest = mujoco.mj_name2id(m, mujoco.mjtObj.mjOBJ_BODY, 'chest')
    falls, limit, peak = False, None, 0.0
    root = d.qpos[:7].copy()
    for t in range(ms * twin.substeps):
        frac = min(1.0, t / (0.6 * ms * twin.substeps))
        for n in twin.nodes:
            if n.a['encoder'] == 'none':
                d.ctrl[n.act] = 0.0; continue
            tgt = pose[n.name]
            if n.name == joint and angle is not None:
                tgt = start[n.name] + (angle - start[n.name]) * frac
            q = d.ten_length[n.ten] if n.tendon else d.qpos[n.qadr]
            dq = d.ten_velocity[n.ten] if n.tendon else d.qvel[n.dadr]
            deliv = min(n.tau_max, (n.kt or 1e9) * n.peak)
            kp = 100.0 if n.tendon else 6.0 * deliv
            kd = 5.0 if n.tendon else 0.04 * kp
            tau = max(-deliv, min(deliv, kp * (tgt - q) - kd * dq))
            d.ctrl[n.act] = tau
            peak = max(peak, abs(tau) / max(deliv, 1e-9))
        if jig:
            d.qpos[:7] = root; d.qvel[:6] = 0
        mujoco.mj_step(m, d)
        up = float(d.xmat[chest][8])
        if up < math.cos(math.radians(45)) or d.qpos[2] < 0.12:
            falls = True
            break
        for n in twin.nodes:
            if n.tendon or n.a['encoder'] == 'none': continue
            q = d.qpos[n.qadr]
            if q < n.a['q_min'] - 0.05 or q > n.a['q_max'] + 0.05:
                limit = n.name
    return dict(falls=falls, joint_limit=limit, person=False, peak=peak, horizon=ms / 1000.0)


def rot_rp(r, p):
    cr, sr, cp, sp = math.cos(r), math.sin(r), math.cos(p), math.sin(p)
    Rx = np.array([[1, 0, 0], [0, cr, -sr], [0, sr, cr]])
    Ry = np.array([[cp, 0, sp], [0, 1, 0], [-sp, 0, cp]])
    return Ry @ Rx


class Recorder:
    """Offscreen MuJoCo render to an mp4, 30 fps of sim time."""

    def __init__(self, twin, path, w=640, h=400):
        self.twin, self.path = twin, path
        self.r = mujoco.Renderer(twin.m, h, w)
        self.frames = []
        self.cam = mujoco.MjvCamera(); self.cam.distance = 1.1; self.cam.elevation = -15; self.cam.azimuth = 135
        self.cam.lookat[:] = twin.d.qpos[:3]

    def maybe_frame(self):
        if self.twin.t_ms % 33: return
        self.cam.lookat[:] = 0.9 * self.cam.lookat + 0.1 * self.twin.d.qpos[:3]
        self.r.update_scene(self.twin.d, self.cam)
        self.frames.append(self.r.render().copy())

    def close(self):
        if not self.frames: return
        import imageio.v3 as iio
        iio.imwrite(self.path, np.stack(self.frames), fps=30, codec='libx264')


def serve(args):
    twin = Twin(args)
    if os.path.exists(args.sock): os.unlink(args.sock)
    s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    s.bind(args.sock)
    print(f'nerves: {args.scene} scene, jig={twin.jig}, faults={sorted(twin.faults) or "none"}, listening on {args.sock}',
          flush=True)
    try:
        while True:
            b, addr = s.recvfrom(65536)
            if b[:1] == b'D':
                tick, relay, frames = wire.down_unpack(b)
                up = twin.tick(relay, frames)
                s.sendto(wire.up_pack(twin.t_ms, twin.imu(), twin.estop_latched, twin.rail_on(),
                                      int(twin.vbat * 1000), up), addr)
            elif b[:1] == b'C':
                msg = json.loads(b[1:].decode())
                if msg.get('cmd') == 'quit':
                    s.sendto(b'c' + json.dumps(dict(ok=True, stats=twin.stats)).encode(), addr)
                    break
                s.sendto(b'c' + json.dumps(twin.control(msg)).encode(), addr)
    finally:
        if twin.recorder: twin.recorder.close()
        s.close()
        if os.path.exists(args.sock): os.unlink(args.sock)


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--sock', default='/tmp/blur_nerves.sock')
    p.add_argument('--scene', default='flat')
    p.add_argument('--jig', action='store_true', help='start clamped in the calibration jig')
    p.add_argument('--jig-height', type=float, default=0.30)
    p.add_argument('--fault', action='append', help='phantom:<joint> forged:<node> forged2:<node> stranger:<node> '
                                                     'flip:<joint> mirror:<joint> dead_enc:<joint> imu_off imu_drop:<ms>')
    p.add_argument('--seed', type=int, default=1, help='assembly + noise seed')
    p.add_argument('--unit', type=int, default=36, help='which physical unit (silicon UIDs)')
    p.add_argument('--latency', type=float, nargs=2, default=[0.1, 0.8], help='per-frame jitter ms (lo hi)')
    p.add_argument('--record', default=None, help='mp4 path')
    serve(p.parse_args())


if __name__ == '__main__':
    main()
