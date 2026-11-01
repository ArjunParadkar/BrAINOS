"""attitude.py -- os/blur/src/attitude.rs, line for line, so a policy trained
here observes the estimate the Teensy computes (gyro integration pulled toward
the accelerometer, trusted only near 1 g), not the simulator's truth.
tests: test_attitude.py holds the two equal on a recorded trace."""
import math
import numpy as np

KP, KI, ACC_GATE, G = 2.0, 0.05, 0.15, 9.80665


def rot_mount(roll, pitch, v):
    cr, sr, cp, sp = math.cos(roll), math.sin(roll), math.cos(pitch), math.sin(pitch)
    a = (cp * v[0] - sp * v[2], v[1], sp * v[0] + cp * v[2])
    return (a[0], cr * a[1] + sr * a[2], -sr * a[1] + cr * a[2])


class Mahony:
    def __init__(self, roll=0.0, pitch=0.0, gyro_bias=(0.0, 0.0, 0.0)):
        self.q = [1.0, 0.0, 0.0, 0.0]
        self.bias = [0.0, 0.0, 0.0]
        self.mount = (roll, pitch, tuple(gyro_bias))
        self.last_w = (0.0, 0.0, 0.0)
        self.trusted = False

    def align(self, acc):
        a = rot_mount(self.mount[0], self.mount[1], acc)
        roll = math.atan2(a[1], a[2]); pitch = math.atan2(-a[0], math.sqrt(a[1] ** 2 + a[2] ** 2))
        cr, sr, cp, sp = math.cos(roll / 2), math.sin(roll / 2), math.cos(pitch / 2), math.sin(pitch / 2)
        self.q = [cr * cp, sr * cp, cr * sp, -sr * sp]
        self.bias = [0.0, 0.0, 0.0]

    def up_body(self):
        w, x, y, z = self.q
        return (2 * (x * z - w * y), 2 * (y * z + w * x), w * w - x * x - y * y + z * z)

    def gravity_body(self):
        u = self.up_body()
        return (-u[0], -u[1], -u[2])

    def update(self, acc, gyro, dt):
        a = rot_mount(self.mount[0], self.mount[1], acc)
        g0 = rot_mount(self.mount[0], self.mount[1], gyro)
        gb = self.mount[2]
        w = [g0[k] - gb[k] - self.bias[k] for k in range(3)]
        self.last_w = tuple(w)
        n = math.sqrt(a[0] ** 2 + a[1] ** 2 + a[2] ** 2)
        self.trusted = n > 0 and abs(n / G - 1.0) < ACC_GATE
        if self.trusted:
            ax = (a[0] / n, a[1] / n, a[2] / n)
            v = self.up_body()
            e = (ax[1] * v[2] - ax[2] * v[1], ax[2] * v[0] - ax[0] * v[2], ax[0] * v[1] - ax[1] * v[0])
            for k in range(3):
                self.bias[k] -= KI * e[k] * dt
                w[k] += KP * e[k]
        qw, qx, qy, qz = self.q
        h = 0.5 * dt
        nq = [qw + h * (-qx * w[0] - qy * w[1] - qz * w[2]), qx + h * (qw * w[0] + qy * w[2] - qz * w[1]),
              qy + h * (qw * w[1] - qx * w[2] + qz * w[0]), qz + h * (qw * w[2] + qx * w[1] - qy * w[0])]
        m = math.sqrt(sum(x * x for x in nq))
        if m > 0 and math.isfinite(m):
            self.q = [x / m for x in nq]

    def tilt_deg(self):
        return math.degrees(math.acos(max(-1.0, min(1.0, self.up_body()[2]))))
