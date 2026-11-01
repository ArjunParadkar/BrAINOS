//! The inner ear: which way is down, from the BMI270.
//!
//! A Mahony complementary filter on the unit quaternion: integrate the
//! gyro, and pull the estimate toward the accelerometer's gravity
//! direction with a proportional-integral correction (the integral is the
//! gyro bias estimate). The accelerometer is trusted only when its
//! magnitude is near 1 g — during an impact or a jump it is measuring the
//! impact, not gravity, and the filter coasts on the gyro.
//!
//! The policies observe `gravity_body()` (unit, body frame) and the
//! bias-corrected rate. `blur/learn/attitude.py` is the same filter line
//! for line, so a policy trained in the twin sees the estimate the Teensy
//! will compute, lag and all — not the simulator's true orientation.

/// Mounting correction from calibration: roll and pitch of the IMU in the
/// chest, and the gyro bias measured at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImuMount {
    pub roll: f32,
    pub pitch: f32,
    pub gyro_bias: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Mahony {
    /// body→world rotation as a unit quaternion (w, x, y, z)
    pub q: [f32; 4],
    pub kp: f32,
    pub ki: f32,
    pub bias: [f32; 3],
    /// [cos, sin] of the mount roll/pitch, applied to every sample
    mount: ImuMount,
    pub accel_trusted: bool,
    last_w: [f32; 3],
}

pub const KP: f32 = 2.0;
pub const KI: f32 = 0.05;
/// trust the accelerometer within this fraction of 1 g
pub const ACC_GATE: f32 = 0.15;

fn rot_mount(m: &ImuMount, v: [f32; 3]) -> [f32; 3] {
    // the sensor reads R_m * v_body; undo it: v_body = R_m^T * v_sensor,
    // R_m = Ry(pitch) * Rx(roll)
    let (cr, sr) = (libm::cosf(m.roll), libm::sinf(m.roll));
    let (cp, sp) = (libm::cosf(m.pitch), libm::sinf(m.pitch));
    // R_m^T = Rx(roll)^T * Ry(pitch)^T
    let a = [cp * v[0] - sp * v[2], v[1], sp * v[0] + cp * v[2]];
    [a[0], cr * a[1] + sr * a[2], -sr * a[1] + cr * a[2]]
}

impl Mahony {
    pub fn new(mount: ImuMount) -> Mahony {
        Mahony {
            q: [1.0, 0.0, 0.0, 0.0],
            kp: KP,
            ki: KI,
            bias: [0.0; 3],
            mount,
            accel_trusted: false,
            last_w: [0.0; 3],
        }
    }

    /// Start from the accelerometer alone (body at rest): roll and pitch
    /// from gravity, yaw zero.
    pub fn align(&mut self, acc_sensor: [f32; 3]) {
        let a = rot_mount(&self.mount, acc_sensor);
        let roll = libm::atan2f(a[1], a[2]);
        let pitch = libm::atan2f(-a[0], libm::sqrtf(a[1] * a[1] + a[2] * a[2]));
        let (cr, sr) = (libm::cosf(roll * 0.5), libm::sinf(roll * 0.5));
        let (cp, sp) = (libm::cosf(pitch * 0.5), libm::sinf(pitch * 0.5));
        self.q = [cr * cp, sr * cp, cr * sp, -sr * sp];
        self.bias = [0.0; 3];
    }

    /// One update: accelerometer (m/s²) and gyro (rad/s) in the sensor
    /// frame, `dt` seconds.
    pub fn update(&mut self, acc_sensor: [f32; 3], gyro_sensor: [f32; 3], dt: f32) {
        let a = rot_mount(&self.mount, acc_sensor);
        let g0 = rot_mount(&self.mount, gyro_sensor);
        let mut w = [
            g0[0] - self.mount.gyro_bias[0] - self.bias[0],
            g0[1] - self.mount.gyro_bias[1] - self.bias[1],
            g0[2] - self.mount.gyro_bias[2] - self.bias[2],
        ];
        self.last_w = w;
        let n = libm::sqrtf(a[0] * a[0] + a[1] * a[1] + a[2] * a[2]);
        self.accel_trusted = n > 0.0 && (n / crate::wire::G - 1.0).abs() < ACC_GATE;
        if self.accel_trusted {
            let ax = [a[0] / n, a[1] / n, a[2] / n];
            // the "up" direction the estimate predicts, in the body frame
            let v = self.up_body();
            // error = measured × predicted
            let e = [ax[1] * v[2] - ax[2] * v[1], ax[2] * v[0] - ax[0] * v[2], ax[0] * v[1] - ax[1] * v[0]];
            for k in 0..3 {
                self.bias[k] -= self.ki * e[k] * dt;
                w[k] += self.kp * e[k];
            }
        }
        let [qw, qx, qy, qz] = self.q;
        let h = 0.5 * dt;
        let nq = [
            qw + h * (-qx * w[0] - qy * w[1] - qz * w[2]),
            qx + h * (qw * w[0] + qy * w[2] - qz * w[1]),
            qy + h * (qw * w[1] - qx * w[2] + qz * w[0]),
            qz + h * (qw * w[2] + qx * w[1] - qy * w[0]),
        ];
        let m = libm::sqrtf(nq[0] * nq[0] + nq[1] * nq[1] + nq[2] * nq[2] + nq[3] * nq[3]);
        if m > 0.0 && m.is_finite() {
            self.q = [nq[0] / m, nq[1] / m, nq[2] / m, nq[3] / m];
        }
    }

    /// World up (0,0,1) expressed in the body frame: the third row of the
    /// body→world rotation.
    pub fn up_body(&self) -> [f32; 3] {
        let [w, x, y, z] = self.q;
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), w * w - x * x - y * y + z * z]
    }

    /// Unit gravity direction in the body frame (what the policies observe).
    pub fn gravity_body(&self) -> [f32; 3] {
        let u = self.up_body();
        [-u[0], -u[1], -u[2]]
    }

    /// Bias-corrected body rate.
    pub fn rate(&self, gyro_sensor: [f32; 3]) -> [f32; 3] {
        let g0 = rot_mount(&self.mount, gyro_sensor);
        [
            g0[0] - self.mount.gyro_bias[0] - self.bias[0],
            g0[1] - self.mount.gyro_bias[1] - self.bias[1],
            g0[2] - self.mount.gyro_bias[2] - self.bias[2],
        ]
    }

    /// The bias-corrected rate from the last update (before the filter's
    /// own correction term): what a policy observes.
    pub fn last_rate(&self) -> [f32; 3] {
        self.last_w
    }

    /// Angle of the chest's up axis from vertical, degrees.
    pub fn tilt_deg(&self) -> f32 {
        let u = self.up_body();
        libm::acosf(u[2].clamp(-1.0, 1.0)) * 180.0 / core::f32::consts::PI
    }
}
