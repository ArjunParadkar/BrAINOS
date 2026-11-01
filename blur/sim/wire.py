"""
wire.py -- the BLUR nerve formats, the Python side.

Mirror of os/blur/src/wire.rs. The two are held together by the golden
vectors in test_wire.py, which are the same bytes the Rust tests assert.
"""
import struct

FC_ESTOP, FC_CENSUS, FC_SETPOINT, FC_FEEDBACK, FC_CONFIG, FC_CONFIG_ACK, FC_IDENTITY, FC_RAW = range(8)
ESTOP_MAGIC = bytes([0xE5, 0x70])
SP_ENABLE = 1
ST_ENABLED, ST_ENC_OK, ST_OVERCURRENT, ST_OVERTEMP, ST_FAULT, ST_ESTOPPED, ST_CALIBRATED, ST_CLAMPED = (1 << k for k in range(8))
KEY = dict(Offset=1, Direction=2, CurrentLimit=3, QMin=4, QMax=5, TurnOffset=6, Commit=7, ProbeTorque=8)
KEY_NAME = {v: k for k, v in KEY.items()}
KIND_ESC1, KIND_ESP32S3 = 1, 2
AMPS_PER_COUNT = 0.01
G = 9.80665
ACC_LSB_PER_G = 4096.0
GYR_LSB_PER_DPS = 16.384
FRAME_BYTES = 12
BITS_WORST_8 = 47 + 64 + (33 + 64) // 4     # 135, as wire.rs CanFrame::wire_bits_worst(8)


def scales(motor):
    if motor == 'winch2204':
        return dict(q=2e-6, tau=0.01, dq=1e-4, kp=20.0, kd=0.2)
    return dict(q=2.5e-4, tau=1e-3, dq=1e-2, kp=0.25, kd=0.01)


def q16(v, s):
    c = v / s
    if c != c:
        return 0
    if c >= 32767: return 32767
    if c <= -32767: return -32767
    return int(c + 0.5) if c >= 0 else -int(-c + 0.5)


def q8(v, s):
    c = v / s
    if c != c or c <= 0: return 0
    if c >= 255: return 255
    return int(c + 0.5)


class Frame:
    __slots__ = ('bus', 'id', 'data')

    def __init__(self, bus, fid, data):
        self.bus, self.id, self.data = bus, fid, bytes(data)[:8]

    @staticmethod
    def make(bus, fc, node, data):
        return Frame(bus, (fc << 7) | (node & 0x7F), data)

    @property
    def fc(self): return self.id >> 7

    @property
    def node(self): return self.id & 0x7F

    def pack(self):
        d = self.data + bytes(8 - len(self.data))
        return struct.pack('<BHB', self.bus, self.id, len(self.data)) + d

    @staticmethod
    def unpack(b):
        bus, fid, n = struct.unpack_from('<BHB', b, 0)
        if bus > 2 or fid >= 0x800 or n > 8:
            return None
        return Frame(bus, fid, b[4:4 + n])

    def __repr__(self):
        return f'Frame(bus={"ABC"[self.bus]}, id=0x{self.id:03x}, {self.data.hex()})'


def setpoint_decode(f, motor):
    if f.fc != FC_SETPOINT or len(f.data) != 8: return None
    s = scales(motor)
    slot, flags, q, t, kp, kd = struct.unpack('<BBhhBB', f.data)
    return dict(slot=slot, flags=flags, q=q * s['q'], tau=t * s['tau'], kp=kp * s['kp'], kd=kd * s['kd'])


def setpoint_encode(bus, node, motor, slot, flags, q, tau, kp, kd):
    s = scales(motor)
    return Frame.make(bus, FC_SETPOINT, node, struct.pack('<BBhhBB', slot, flags, q16(q, s['q']), q16(tau, s['tau']),
                                                          q8(kp, s['kp']), q8(kd, s['kd'])))


def feedback_encode(bus, node, motor, slot, status, q, dq, current):
    s = scales(motor)
    return Frame.make(bus, FC_FEEDBACK, node, struct.pack('<BBhhh', slot, status, q16(q, s['q']), q16(dq, s['dq']),
                                                          q16(current, AMPS_PER_COUNT)))


def raw_encode(bus, node, slot, status, counts, turns, temp_c):
    return Frame.make(bus, FC_RAW, node, struct.pack('<BBHhbB', slot, status, counts & 0xFFFF,
                                                     max(-32768, min(32767, turns)), int(temp_c), 0))


def config_decode(f):
    if f.fc not in (FC_CONFIG, FC_CONFIG_ACK) or len(f.data) != 8: return None
    slot, key, value, seq, _ = struct.unpack('<BBfBB', f.data)
    return dict(slot=slot, key=key, value=value, seq=seq, ack=f.fc == FC_CONFIG_ACK)


def config_ack(bus, node, slot, key, value, seq):
    return Frame.make(bus, FC_CONFIG_ACK, node, struct.pack('<BBfBB', slot, key, value, seq, 0))


def identity_pages(bus, node, kind, slots, fw, uid12):
    return [Frame.make(bus, FC_IDENTITY, node, bytes([p, kind, slots, fw]) + uid12[4 * p:4 * p + 4]) for p in range(3)]


def is_estop(f):
    return f.id == 0 and len(f.data) >= 2 and f.data[:2] == ESTOP_MAGIC


def bmi270_pack(acc_i16, gyr_i16, sensortime):
    return struct.pack('<hhhhhh', *acc_i16, *gyr_i16) + struct.pack('<I', sensortime & 0xFFFFFF)[:3]


def down_unpack(b):
    """'D' tick:u32 relay:u8 n:u8 frames"""
    if len(b) < 7 or b[0:1] != b'D': return None
    tick, relay, n = struct.unpack_from('<IBB', b, 1)
    frames = []
    for k in range(n):
        f = Frame.unpack(b[7 + k * FRAME_BYTES: 7 + (k + 1) * FRAME_BYTES])
        if f is not None:
            frames.append(f)
    return tick, bool(relay), frames


def up_pack(tick, imu_bytes, estop_latched, rail_on, vbat_mv, frames):
    flags = (1 if imu_bytes is not None else 0) | (2 if estop_latched else 0) | (4 if rail_on else 0)
    imu = imu_bytes if imu_bytes is not None else bytes(15)
    frames = frames[:96]
    return (b'U' + struct.pack('<IBH', tick, flags, vbat_mv) + imu + bytes([len(frames)])
            + b''.join(f.pack() for f in frames))
