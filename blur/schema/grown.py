#!/usr/bin/env python3
"""
grown.py -- B4's grown reflex: gaze lock from the body schema, learned the
way the vestibulo-ocular reflex is, by cancelling retinal slip.

Nothing here uses ground truth. From the babbling log alone:
  1. the ORIENTATION module(s) are the ones the schema model assigned the
     accelerometer channels to (chosen by what the body's own channels are,
     not by any decoding against truth);
  2. a local linear slip model, fitted by least squares over consecutive
     camera frames (0.1 s apart), where retinal slip is the image's own
     frame-to-frame shift (phase correlation, in pixels: what a retina
     measures):
         slip ~ J_b d(z_orient) + J_n d(neck encoders)
     (v1 used the change in the camera's principal components as "slip";
     that model explained R^2 = 0.006 of it and was abandoned before any
     test was run.)
  3. the reflex cancels the predicted slip:
         d(neck) = -pinv(J_n) J_b d(z_orient),
     integrated, with a 2 s leak back to the neutral neck (fixed in advance,
     not tuned on test episodes).
At run time it sees only the orientation latent (computed by the schema
model's own encoders from a 32-sample window of the body's channels) and the
neck encoders.
"""
import os, sys, json, math
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'learn'))
import numpy as np
import torch
import env as E
from schema_model import Schema, load, WIN, CAM_PCS

LEAK_S = 2.0
NECK = [E.NAMES.index(n) for n in ('neck_lift', 'neck_pan', 'neck_tilt')]


def phase_shift(a, b):
    """(dy, dx) image translation from a to b by phase correlation, pixels"""
    A = np.fft.fft2(a - a.mean()); B = np.fft.fft2(b - b.mean())
    R = A.conj() * B; R /= np.abs(R) + 1e-9
    r = np.fft.ifft2(R).real
    dy, dx = np.unravel_index(np.argmax(r), r.shape)
    h, w = r.shape
    return np.array([dy - h if dy > h // 2 else dy, dx - w if dx > w // 2 else dx], float)


class Grown:
    def __init__(self, run, data=os.path.join(HERE, 'data', 'babble_s0.npz')):
        ck = torch.load(os.path.join(run, 'model.pt'), map_location='cpu', weights_only=False)
        self.args = ck['args']; ch = ck['channels']
        X, ch2, group, gt, info = load(data)
        assert ch2 == ch
        self.mu, self.sd, self.keep = info['mu'], info['sd'], info['keep']
        self.ch = ch
        C = len(ch)
        n_cmd = int(np.sum(group == 0)) if self.args.get('variant') == 'fwd' else 0
        net = Schema(C, self.args['K'], self.args['d'], n_cmd=n_cmd)
        net.load_state_dict(ck['net']); net.eval(); net.tau = 0.05
        self.net = net
        A = net.assignment().detach()
        self.hard = torch.nn.functional.one_hot(A.argmax(1), net.K).float()
        hk = A.argmax(1).numpy()
        self.orient = sorted({int(hk[i]) for i, c in enumerate(ch) if c in ('imu:ax', 'imu:ay', 'imu:az')})
        # the camera basis the model was trained with
        z = np.load(data)
        cam = z['cam'].astype(np.float32) / 255.0
        F = cam.reshape(len(cam), -1)
        self.cam_mean = F.mean(0)
        U, S, Vt = np.linalg.svd(F - self.cam_mean, full_matrices=False)
        self.cam_V = Vt[:CAM_PCS]
        # 2. the slip model, over camera frames
        Zo = self.encode_all(X)                                    # latents for every sample
        idx = z['cam_idx']; idx = idx[idx >= WIN - 1]
        k = idx - (WIN - 1)
        frames = z['cam'][np.searchsorted(z['cam_idx'], idx)].astype(np.float32).mean(-1)
        dC = np.array([phase_shift(frames[i], frames[i + 1]) for i in range(len(frames) - 1)])
        neck = z['q'][:, NECK][idx]
        dZ = np.diff(Zo[k], axis=0); dN = np.diff(neck, axis=0)
        M = np.concatenate([dZ, dN], 1)
        sol, *_ = np.linalg.lstsq(M, dC, rcond=None)
        pred = M @ sol
        self.slip_r2 = float(1 - ((dC - pred) ** 2).sum() / ((dC - dC.mean(0)) ** 2).sum())
        nz = dZ.shape[1]
        Jb, Jn = sol[:nz].T, sol[nz:].T                            # (16 x nz), (16 x 3)
        self.G = -np.linalg.pinv(Jn) @ Jb                          # (3 x nz)
        self.info = dict(orientation_modules=self.orient, slip_model_r2=self.slip_r2, gain_norm=float(np.linalg.norm(self.G)))

    def encode_all(self, X):
        out = []
        with torch.no_grad():
            Xt = torch.as_tensor(X)
            for b in range(WIN - 1, len(X), 4096):
                ii = torch.arange(b, min(b + 4096, len(X)))
                wi = ii[:, None] + torch.arange(-WIN + 1, 1)[None, :]
                z = self.net.encode(Xt[wi], self.hard)
                out.append(z[:, self.orient].reshape(len(ii), -1).numpy())
        return np.concatenate(out)

    # ---- the run-time reflex (gaze.py's controller interface)
    def reset(self, sc):
        self.buf = []
        self.integ = np.zeros(3)
        self.z_prev = None
        self.pcs = np.zeros(CAM_PCS)
        self.k = 0
        import mujoco
        self.rr = mujoco.Renderer(sc.m, 48, 64)
        self.eye = mujoco.mj_name2id(sc.m, mujoco.mjtObj.mjOBJ_CAMERA, 'eye')
        self.last_cmd = sc.e.d.ctrl[sc.e.act].copy()

    def observe(self, sc):
        e = sc.e
        if self.k % 5 == 0:
            self.rr.update_scene(e.d, self.eye)
            img = self.rr.render().astype(np.float32).reshape(-1) / 255.0
            self.pcs = (img - self.cam_mean) @ self.cam_V.T
        self.k += 1
        acc, gyr = e.imu_sample()
        row = np.concatenate([e.d.ctrl[e.act], e.q(), e.d.actuator_force[e.act] / E.TAU, acc, gyr, self.pcs])
        row = row[self.keep]
        self.buf.append((row - self.mu) / self.sd)
        self.buf = self.buf[-WIN:]
        if len(self.buf) < WIN:
            return
        with torch.no_grad():
            x = torch.as_tensor(np.stack(self.buf)[None].astype(np.float32))
            z = self.net.encode(x, self.hard)[0, self.orient].reshape(-1).numpy()
        if self.z_prev is not None:
            self.integ += self.G @ (z - self.z_prev) - self.integ * 0.02 / LEAK_S
        self.z_prev = z

    def neck(self, sc):
        return E.QSTAND[NECK] + self.integ


if __name__ == '__main__':
    g = Grown(sys.argv[1])
    print(json.dumps(g.info))
