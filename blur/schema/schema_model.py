#!/usr/bin/env python3
"""
schema_model.py -- B2: discover BLUR's body schema from its babbling.
A model "inspired by OMEN (Claudi & Fiete)": OMEN's temporal-CNN encoder,
none of its fixed attractor geometry.

Input channels (all z-scored; the model is told nothing about what they are):
  efference copy (26 joint targets), encoders (26), motor currents (26),
  IMU (3 specific force + 3 rate), camera (16 principal components of the
  head camera's frames, held between 10 Hz frames).

Model:
  * K modules, each with an UNCONSTRAINED Euclidean latent z_k in R^d
    (no ring, no torus, no sphere, no periodic coordinate anywhere);
  * a learned soft assignment A (channels x modules, softmax per channel,
    temperature annealed toward a hard assignment); module k's encoder sees
    only its channels (x * A[:, k]) through OMEN's tCNN over a 32-sample
    (0.64 s) window; its latent is standardised per dimension (BatchNorm,
    no affine: linear, so it cannot change topology) and lightly noised (a
    bottleneck);
  * a forward model: every channel at t+1 is predicted only from the
    latents of the modules it is assigned to, x_hat = sum_k A[:,k] * D_k(z_k);
  * an entropy penalty pushes each channel toward one module, and a
    group-lasso penalty on module usage prunes modules the data does not need.

  python schema_model.py data/babble_s0.npz runs/m0 [--K 16 --d 6 --epochs 80]
Writes runs/m0/: model.pt, latents.npz (z for every sample, assignment,
channel names, ground truth carried through for evaluation), train.csv.
"""
import os, sys, json, math, argparse, time, csv
for _v in ('OMP_NUM_THREADS', 'OPENBLAS_NUM_THREADS', 'MKL_NUM_THREADS'):
    os.environ.setdefault(_v, '1')
import numpy as np
import torch, torch.nn as nn
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
from omen_tcnn import tCNN, make_mlp

WIN = 32
CAM_PCS = 16


def load(path):
    z = np.load(path)
    names = [str(n) for n in z['names']]
    n = len(z['q'])
    # camera: PCA of the frames, held between frames
    cam = z['cam'].astype(np.float32) / 255.0
    # PCA on the GPU (a CPU SVD here would compete with the machine's research jobs)
    dev = 'cuda' if torch.cuda.is_available() else 'cpu'
    F = torch.as_tensor(cam.reshape(len(cam), -1), device=dev)
    F = F - F.mean(0)
    U, S, Vt = torch.linalg.svd(F, full_matrices=False)
    pcs = (F @ Vt[:CAM_PCS].T).cpu().numpy()
    S = S.cpu().numpy()
    held = np.zeros((n, CAM_PCS), np.float32)
    idx = z['cam_idx']
    for j in range(len(idx)):
        held[idx[j]: idx[j + 1] if j + 1 < len(idx) else n] = pcs[j]
    X = np.concatenate([z['cmd'], z['q'], z['cur'], z['imu'], held], 1).astype(np.float32)
    ch = ([f'cmd:{a}' for a in names] + [f'enc:{a}' for a in names] + [f'cur:{a}' for a in names]
          + ['imu:ax', 'imu:ay', 'imu:az', 'imu:gx', 'imu:gy', 'imu:gz'] + [f'cam:pc{i}' for i in range(CAM_PCS)])
    group = [0] * 26 + [1] * 26 + [2] * 26 + [3] * 6 + [4] * CAM_PCS
    keep = X.std(0) > 1e-6
    X = X[:, keep]; ch = [c for c, k in zip(ch, keep) if k]; group = np.array([g for g, k in zip(group, keep) if k])
    mu, sd = X.mean(0), X.std(0)
    X = (X - mu) / sd
    gt = dict(chest_quat=z['chest_quat'], head_xmat=z['head_xmat'], q=z['q'], gimbal_quat=z['gimbal_quat'])
    return X, ch, group, gt, dict(mu=mu, sd=sd, keep=keep, cam_var=(S[:CAM_PCS] ** 2 / (S ** 2).sum()).tolist())


class Schema(nn.Module):
    def __init__(self, C, K, d, tau0=1.0, n_cmd=0):
        super().__init__()
        self.K, self.d = K, d
        self.n_cmd = n_cmd            # fwd variant: decoders also see the module's FUTURE efference copy
        self.hard = False
        self.A_logits = nn.Parameter(0.01 * torch.randn(C, K))
        self.enc = nn.ModuleList([tCNN(n_inputs=C, n_hidden=64, n_layers=1, n_outputs=d, kernel_size=11, n_kernels=2,
                                       activation='relu', n_conv_groups=-1, dilation=2, dropout=0.1) for _ in range(K)])
        self.norm = nn.ModuleList([nn.BatchNorm1d(d, affine=False) for _ in range(K)])
        self.dec = nn.ModuleList([make_mlp(d + n_cmd, 128, 2, C, 'relu', dropout=0.0) for _ in range(K)])
        self.tau = tau0

    def assignment(self):
        soft = torch.softmax(self.A_logits / self.tau, dim=1)
        if not self.hard:
            return soft
        # straight-through: each channel belongs to exactly one module in the
        # forward pass (a module sees only its own channels), soft gradient back
        hard = torch.nn.functional.one_hot(soft.argmax(1), self.K).float()
        return hard + soft - soft.detach()

    def encode(self, x, A=None):
        """x: (B, T, C) -> z: (B, K, d) at the window's last sample"""
        A = self.assignment() if A is None else A
        zs = []
        for k in range(self.K):
            h = self.enc[k](x * A[:, k])[:, -1, :]
            zs.append(self.norm[k](h))
        return torch.stack(zs, 1)

    def forward(self, x, noise=0.0, cmd_future=None):
        A = self.assignment()
        z = self.encode(x, A)
        if noise > 0:
            z = z + noise * torch.randn_like(z)
        out = 0
        for k in range(self.K):
            inp = z[:, k]
            if self.n_cmd:
                inp = torch.cat([inp, cmd_future * A[:self.n_cmd, k]], 1)
            out = out + A[:, k] * self.dec[k](inp)
        return out, z, A


def windows(X, idx):
    return np.stack([X[i - WIN + 1:i + 1] for i in idx])


def main():
    p = argparse.ArgumentParser()
    p.add_argument('data'); p.add_argument('out')
    p.add_argument('--K', type=int, default=16); p.add_argument('--d', type=int, default=6)
    p.add_argument('--epochs', type=int, default=80); p.add_argument('--batch', type=int, default=512)
    p.add_argument('--lr', type=float, default=2e-3); p.add_argument('--noise', type=float, default=0.05)
    p.add_argument('--ent', type=float, default=0.02); p.add_argument('--use', type=float, default=0.002)
    p.add_argument('--seed', type=int, default=0)
    p.add_argument('--variant', default='ae', choices=['ae', 'fwd'],
                   help='ae: predict every channel 1 sample ahead (v1). fwd: predict sensors 200 ms ahead given the '
                        "module's future efference copy, hard straight-through assignment after a soft warm-up")
    p.add_argument('--horizon', type=int, default=10, help='fwd: samples ahead (10 = 200 ms)')
    p.add_argument('--warmup', type=int, default=10, help='fwd: soft epochs before the assignment goes hard')
    p.add_argument('--logit-lr', type=float, default=None, help='learning rate for the assignment logits (default: --lr)')
    p.add_argument('--assign', default=None, help='npz with fixed module labels per channel (anchor_groups.py); routing is not learned')
    p.add_argument('--shuffle', action='store_true', help='negative control: roll every channel by its own random offset (keeps each channel, destroys every relation between channels)')
    a = p.parse_args()
    torch.manual_seed(a.seed); np.random.seed(a.seed); torch.set_num_threads(1)
    os.makedirs(a.out, exist_ok=True)
    dev = 'cuda' if torch.cuda.is_available() else 'cpu'
    X, ch, group, gt, info = load(a.data)
    if a.shuffle:
        rs = np.random.default_rng(1234)
        X = np.stack([np.roll(X[:, c], int(rs.integers(len(X) // 10, len(X) - len(X) // 10))) for c in range(X.shape[1])], 1)
    N, C = X.shape
    # each sensor family weighs the same in the loss, however many channels it has
    w = np.array([1.0 / np.sum(group == g) for g in group], np.float32); w = w / w.mean()
    W = torch.as_tensor(w, device=dev)
    fwd = a.variant == 'fwd'
    H = a.horizon if fwd else 1
    n_cmd = int(np.sum(group == 0)) if fwd else 0          # efference channels come first
    if fwd:
        W = W * torch.as_tensor((group != 0).astype(np.float32), device=dev)   # commands are given, not predicted
        W = W / W.mean()
    idx = np.arange(WIN - 1, N - H)
    split = int(0.8 * len(idx))
    tr, va = idx[:split], idx[split + WIN:]
    Xt = torch.as_tensor(X, device=dev)
    def batch(ii):
        ti = torch.as_tensor(ii, device=dev)
        wi = ti[:, None] + torch.arange(-WIN + 1, 1, device=dev)[None, :]
        y = Xt[ti + H]
        return Xt[wi], y, (y[:, :n_cmd] if fwd else None)
    if a.assign:
        lab = np.load(a.assign)['labels']; a.K = int(lab.max()) + 1
    net = Schema(C, a.K, a.d, n_cmd=n_cmd).to(dev)
    if a.assign:
        with torch.no_grad():
            net.A_logits.copy_(torch.as_tensor(np.eye(a.K)[lab] * 50.0, dtype=torch.float32))
        net.A_logits.requires_grad_(False)
    rest = [q for n_, q in net.named_parameters() if n_ != 'A_logits']
    groups = [dict(params=rest)] + ([] if a.assign else [dict(params=[net.A_logits], lr=a.logit_lr or a.lr, weight_decay=0.0)])
    opt = torch.optim.AdamW(groups, lr=a.lr, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, a.epochs)
    log = csv.writer(open(os.path.join(a.out, 'train.csv'), 'w', newline=''))
    log.writerow(['epoch', 'tau', 'train_mse', 'val_mse', 'val_mse_by_group', 'assign_entropy', 'active_modules', 'crispness'])
    t0 = time.time()
    for ep in range(a.epochs):
        net.tau = 1.0 * (0.05 / 1.0) ** (ep / max(1, a.epochs - 1))
        net.hard = fwd and (ep >= a.warmup or bool(a.assign))
        net.train()
        perm = np.random.permutation(tr); tl = []
        for b in range(0, len(perm) - a.batch + 1, a.batch):
            x, y, cf = batch(perm[b:b + a.batch])
            out, z, A = net(x, noise=a.noise, cmd_future=cf)
            mse = (((out - y) ** 2) * W).mean()
            As = torch.softmax(net.A_logits / net.tau, dim=1)      # penalties on the soft assignment (A may be hard)
            ent = -(As * torch.log(As + 1e-9)).sum(1).mean()
            use = torch.sqrt(As.sum(0) + 1e-6).sum()
            loss = mse + a.ent * ent + a.use * use
            opt.zero_grad(); loss.backward(); opt.step(); tl.append(float(mse.detach()))
        sched.step()
        net.eval()
        with torch.no_grad():
            vs, vg = [], np.zeros(5); cnt = 0
            for b in range(0, len(va), 2048):
                x, y, cf = batch(va[b:b + 2048])
                out, _, A = net(x, cmd_future=cf)
                e = ((out - y) ** 2)
                vs.append(float((e * W).mean()) * len(x)); cnt += len(x)
                for g in range(5):
                    vg[g] += float(e[:, torch.as_tensor(group == g, device=dev)].mean()) * len(x)
            hard = A.argmax(1).cpu().numpy()
            crisp = float(torch.softmax(net.A_logits / net.tau, 1).max(1).values.mean())
            active = len(set(hard.tolist()))
            ent = float(-(A * torch.log(A + 1e-9)).sum(1).mean())
        row = [ep, round(net.tau, 4), round(np.mean(tl), 4), round(sum(vs) / cnt, 4), json.dumps([round(v / cnt, 3) for v in vg]), round(ent, 3), active, round(crisp, 3)]
        log.writerow(row)
        print(f'[{time.time() - t0:5.0f}s] ' + ' '.join(map(str, row)), flush=True)
    # latents for every sample, with the hard assignment
    net.eval()
    with torch.no_grad():
        A = net.assignment()
        Ah = torch.nn.functional.one_hot(A.argmax(1), a.K).float()
        Z = []
        allidx = np.arange(WIN - 1, N)
        for b in range(0, len(allidx), 2048):
            wi = torch.as_tensor(allidx[b:b + 2048], device=dev)[:, None] + torch.arange(-WIN + 1, 1, device=dev)[None, :]
            Z.append(net.encode(Xt[wi], Ah).cpu().numpy())
        Z = np.concatenate(Z)
    torch.save(dict(net=net.state_dict(), args=vars(a), channels=ch), os.path.join(a.out, 'model.pt'))
    np.savez_compressed(os.path.join(a.out, 'latents.npz'), Z=Z, sample=allidx, A=A.cpu().numpy(), channels=np.array(ch),
                        group=group, **{f'gt_{k}': v[allidx] for k, v in gt.items()}, split=split + WIN - 1)
    json.dump(dict(args=vars(a), cam_pc_var=info['cam_var'], channels=ch), open(os.path.join(a.out, 'info.json'), 'w'), indent=1)
    print('wrote', a.out, flush=True)


if __name__ == '__main__':
    main()
