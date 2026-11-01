#!/usr/bin/env python3
"""
ppo.py -- PPO for BLUR's skills, CPU MuJoCo in worker processes, a small
torch MLP in the main process.

  python ppo.py --task getup --hours 2 --out runs/getup_v1

Writes runs/<name>/: progress.csv (one row per iteration), policy.pt
(latest), policy_best.pt, and policy.json -- the exported actor (weights,
observation normaliser, action scale, the obs layout), which is what the
Rust motor cortex and the twin's validation load.
"""
import argparse, csv, json, math, multiprocessing as mp, os, sys, time
os.environ.setdefault('OMP_NUM_THREADS', '1')
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)


def registry():
    import tasks as T, skills as S, dayone as D
    return {**T.TASKS, **S.SKILLS, **D.DAYONE}


# ---------------------------------------------------------------- exact resume
# A checkpoint is taken at an iteration boundary. Each worker CANONICALISES
# its environments there: it reads MuJoCo's integration state
# (mj_getState(mjSTATE_INTEGRATION): time, qpos, qvel, act, warm start, ctrl,
# applied forces, equality flags, mocap) and immediately writes it back with
# mj_setState + mj_forward, so the derived quantities (sensors, tendon lengths,
# actuator forces) are recomputed from the state. The run that keeps going and
# the run resumed from the file then continue from bit-identical states. The
# rest of an env (its RNG, task, attitude filter, randomised model arrays) is
# pickled; the MjModel itself is not (60 MB of meshes per env): only the arrays
# that differ from a freshly built model are stored.
def _model_arrays(m):
    return [k for k in dir(m) if not k.startswith('_') and isinstance(getattr(m, k, None), np.ndarray)]


def env_state(e, base, names):
    import mujoco, pickle
    spec = mujoco.mjtState.mjSTATE_INTEGRATION
    st = np.zeros(mujoco.mj_stateSize(e.m, spec))
    mujoco.mj_getState(e.m, e.d, st, spec)
    mujoco.mj_setState(e.m, e.d, st, spec); mujoco.mj_forward(e.m, e.d)       # canonicalise
    py = pickle.dumps({k: v for k, v in e.__dict__.items() if k not in ('m', 'd')})
    md = {k: getattr(e.m, k).copy() for k in names if not np.array_equal(getattr(e.m, k), getattr(base, k))}
    return dict(state=st, py=py, model=md, opt_timestep=e.m.opt.timestep)


def env_restore(e, saved, base, names):
    import mujoco, pickle
    e.__dict__.update(pickle.loads(saved['py']))
    for k in names:
        want = saved['model'].get(k, getattr(base, k))
        cur = getattr(e.m, k)
        if not np.array_equal(cur, want):
            cur[...] = want
    e.m.opt.timestep = saved['opt_timestep']
    spec = mujoco.mjtState.mjSTATE_INTEGRATION
    mujoco.mj_setState(e.m, e.d, saved['state'], spec); mujoco.mj_forward(e.m, e.d)


def worker(conn, task_name, task_kw, seed, n_env):
    import env as E, tasks as T
    envs = [E.BlurEnv(registry()[task_name](**task_kw), seed=seed * 1000 + k) for k in range(n_env)]
    obs = [e.reset() for e in envs]
    ep_ret = [0.0] * n_env; ep_len = [0] * n_env
    base = E.build_scene('flat', terrain=envs[0].terrain >= 0); names = _model_arrays(base)
    conn.send(np.stack(obs))
    while True:
        cmd, data = conn.recv()
        if cmd == 'step':
            out_o, out_r, out_d, fin, fin_idx = [], [], [], [], []
            for k, e in enumerate(envs):
                o, r, d, info = e.step(data[k])
                ep_ret[k] += r; ep_len[k] += 1
                if d:
                    succ = T.success_standing(e) if task_name == 'getup' else e.task.success(e)
                    fin.append((ep_ret[k], ep_len[k], float(succ), getattr(e.task, 'kind', ''), float(e.up() > 0.9 and e.d.qpos[2] > 0.2)))
                    fin_idx.append(k)
                    o = e.reset(); ep_ret[k] = 0.0; ep_len[k] = 0
                out_o.append(o); out_r.append(r); out_d.append(d)
            conn.send((np.stack(out_o), np.array(out_r, np.float32), np.array(out_d), fin, fin_idx))
        elif cmd == 'save':
            conn.send(dict(envs=[env_state(e, base, names) for e in envs], ep_ret=list(ep_ret), ep_len=list(ep_len)))
            continue
        elif cmd == 'load':
            for e, sv in zip(envs, data['envs']):
                env_restore(e, sv, base, names)
            ep_ret[:] = data['ep_ret']; ep_len[:] = data['ep_len']
            conn.send(True)
            continue
        elif cmd in ('severity', 'level'):
            for e in envs:
                e.task.level = data
                if hasattr(e.task, 'severity'):
                    e.task.severity = data
            conn.send(True)
        elif cmd == 'close':
            return


class RunningNorm:
    def __init__(self, n):
        self.mean = np.zeros(n); self.var = np.ones(n); self.count = 1e-4

    def update(self, x):
        bm, bv, bc = x.mean(0), x.var(0), x.shape[0]
        delta = bm - self.mean; tot = self.count + bc
        self.mean = self.mean + delta * bc / tot
        self.var = (self.var * self.count + bv * bc + delta ** 2 * self.count * bc / tot) / tot
        self.count = tot

    def __call__(self, x):
        return np.clip((x - self.mean) / np.sqrt(self.var + 1e-8), -10, 10)


def build_net(obs_dim, act_dim, hidden=(256, 256, 128), init_std=0.5):
    import torch, torch.nn as nn

    class AC(nn.Module):
        def __init__(self):
            super().__init__()
            def mlp(out):
                layers, d = [], obs_dim
                for h in hidden:
                    layers += [nn.Linear(d, h), nn.ELU()]; d = h
                layers.append(nn.Linear(d, out))
                return nn.Sequential(*layers)
            self.pi = mlp(act_dim); self.v = mlp(1)
            self.log_std = nn.Parameter(torch.full((act_dim,), float(np.log(init_std))))
            with torch.no_grad():
                self.pi[-1].weight.mul_(0.01); self.pi[-1].bias.zero_()

        def dist(self, o):
            mu = self.pi(o)
            return torch.distributions.Normal(mu, self.log_std.exp().expand_as(mu))

    return AC()


def export(net, norm, path, meta):
    import torch
    layers = []
    lin = [m for m in net.pi if isinstance(m, torch.nn.Linear)]
    for m in lin:
        layers.append(dict(w=m.weight.detach().numpy().tolist(), b=m.bias.detach().numpy().tolist()))
    json.dump(dict(meta, layers=layers, activation='elu', obs_mean=norm.mean.tolist(),
                   obs_std=np.sqrt(norm.var + 1e-8).tolist(), obs_clip=10.0), open(path, 'w'))


def main():
    import torch
    p = argparse.ArgumentParser()
    p.add_argument('--task', default='getup')
    p.add_argument('--task-kw', default='{}')
    p.add_argument('--workers', type=int, default=16)
    p.add_argument('--envs', type=int, default=4)
    p.add_argument('--steps', type=int, default=96)
    p.add_argument('--hours', type=float, default=2.0)
    p.add_argument('--lr', type=float, default=3e-4)
    p.add_argument('--out', default='runs/getup')
    p.add_argument('--resume', default=None)
    p.add_argument('--seed', type=int, default=0)
    p.add_argument('--curriculum', action='store_true')
    p.add_argument('--severity0', type=float, default=0.3)
    # job-system options (blur/hpcc/lane.sbatch): exact checkpoint/resume
    p.add_argument('--iters', type=int, default=0, help='stop after this many iterations (0 = only --hours)')
    p.add_argument('--ckpt-min', type=float, default=15.0, help='checkpoint every N wall minutes')
    p.add_argument('--ckpt-iters', type=int, default=0, help='also checkpoint every N iterations (tests)')
    p.add_argument('--deadline', type=float, default=0.0, help='unix time: checkpoint and exit 75 when reached')
    p.add_argument('--auto-resume', action='store_true', help='continue from <out>/ckpt.pt if it exists')
    p.add_argument('--hard-stop-after', type=int, default=0, help='TEST ONLY: exit at this iteration without saving')
    a = p.parse_args()
    torch.set_num_threads(4)
    torch.manual_seed(a.seed); np.random.seed(a.seed)
    os.makedirs(a.out, exist_ok=True)
    task_kw = json.loads(a.task_kw)

    ctx = mp.get_context('fork')
    pipes, procs = [], []
    for w in range(a.workers):
        c1, c2 = ctx.Pipe()
        pr = ctx.Process(target=worker, args=(c2, a.task, task_kw, a.seed * 100 + w + 1, a.envs), daemon=True)
        pr.start(); pipes.append(c1); procs.append(pr)
    obs = np.concatenate([c.recv() for c in pipes])
    N, obs_dim = obs.shape
    # env 0 of every other worker acts on the policy's MEAN action: its
    # episodes are the deterministic success the curriculum and the best
    # checkpoint are judged by; its transitions are masked out of the update
    det = np.zeros(N, bool)
    det[[w * a.envs for w in range(0, a.workers, 2)]] = True
    det_t = torch.as_tensor(det)
    import env as E
    act_dim = E.NA
    net = build_net(obs_dim, act_dim, init_std=getattr(registry()[a.task], 'init_std', 0.5))
    norm = RunningNorm(obs_dim)
    if a.resume:
        ck = torch.load(a.resume, weights_only=False)
        net.load_state_dict(ck['net']); norm.__dict__.update(ck['norm'])
    opt = torch.optim.Adam(net.parameters(), lr=a.lr)
    ckpt_path = os.path.join(a.out, 'ckpt.pt')
    resume = torch.load(ckpt_path, weights_only=False) if a.auto_resume and os.path.exists(ckpt_path) else None
    gamma, lam, clip, epochs, n_mb = 0.99, 0.95, 0.2, 5, 4
    import tasks as Tk
    meta = dict(task=a.task, obs_dim=obs_dim, act_dim=act_dim, action_scale=E.ACTION_SCALE,
                action_filter=getattr(registry()[a.task], 'action_filter', 1.0),
                action_mode=getattr(registry()[a.task], 'action_mode', 'offset'), attitude='mahony (learn/attitude.py)',
                policy_joints=[E.NAMES[i] for i in E.POLICY_JOINTS], control_dt=E.CONTROL_DT,
                obs_layout='gravity_imu(3), 0.25*gyro_imu(3), q-nominal[policy](19), 0.05*dq[policy](19), prev_action(19), command')

    header = ['iter', 'wall_s', 'steps', 'ret_mean', 'ep_len', 'success', 'success_push', 'success_fallen', 'std', 'kl', 'lr', 'severity', 'success_det', 'n_det']
    prog = os.path.join(a.out, 'progress.csv')
    keep = []
    if resume is not None and os.path.exists(prog):
        # rows written after the checkpoint belong to the lost segment
        keep = [r for r in list(csv.reader(open(prog)))[1:] if r and int(r[0]) <= resume['it']]
    f = open(prog, 'w', newline=''); wr = csv.writer(f)
    wr.writerow(header); wr.writerows(keep); f.flush()
    severity = a.severity0 if a.curriculum else 1.0
    if a.curriculum and resume is None:
        for c in pipes:
            c.send(('severity', severity))
        for c in pipes:
            c.recv()
    t_start = time.time(); total = 0; it = 0; best = -1
    recent = []; det_win = []
    # the budget is ACTIVE time: yield_guard.sh may freeze this process while
    # the machine's research jobs run; a gap longer than 2 min is not spent
    active = 0.0; t_last = time.time()
    if resume is not None:
        net.load_state_dict(resume['net']); opt.load_state_dict(resume['opt']); norm.__dict__.update(resume['norm'])
        it, total, active, best, severity = resume['it'], resume['total'], resume['active'], resume['best'], resume['severity']
        recent, det_win, obs = resume['recent'], resume['det_win'], resume['obs']
        torch.set_rng_state(resume['torch_rng']); np.random.set_state(resume['np_rng'])
        for c, w in zip(pipes, resume['workers']):
            c.send(('load', w))
        for c in pipes:
            c.recv()
        print(f'resumed {ckpt_path} at iter {it}, {total} steps, {active / 3600:.2f} active h', flush=True)

    def checkpoint():
        for c in pipes:
            c.send(('save', None))
        workers = [c.recv() for c in pipes]
        ck = dict(net=net.state_dict(), opt=opt.state_dict(), norm=dict(norm.__dict__), it=it, total=total,
                  active=active, best=best, severity=severity, recent=recent, det_win=det_win, obs=obs,
                  torch_rng=torch.get_rng_state(), np_rng=np.random.get_state(), workers=workers,
                  args=vars(a), saved=time.time())
        torch.save(ck, ckpt_path + '.tmp'); os.replace(ckpt_path + '.tmp', ckpt_path)

    t_ck = time.time()
    while active < a.hours * 3600 and not (a.iters and it >= a.iters):
        now = time.time(); active += min(now - t_last, 120.0); t_last = now
        T = a.steps
        B_o = np.zeros((T, N, obs_dim), np.float32); B_a = np.zeros((T, N, act_dim), np.float32)
        B_lp = np.zeros((T, N), np.float32); B_r = np.zeros((T, N), np.float32)
        B_d = np.zeros((T, N), np.float32); B_v = np.zeros((T + 1, N), np.float32)
        fins = []
        for t in range(T):
            norm.update(obs)
            on = torch.as_tensor(norm(obs), dtype=torch.float32)
            with torch.no_grad():
                dist = net.dist(on); act = dist.sample(); v = net.v(on).squeeze(-1)
                act[det_t] = dist.mean[det_t]
                lp = dist.log_prob(act).sum(-1)
            B_o[t] = norm(obs); B_a[t] = act.numpy(); B_lp[t] = lp.numpy(); B_v[t] = v.numpy()
            an = act.numpy()
            for w, c in enumerate(pipes):
                c.send(('step', an[w * a.envs:(w + 1) * a.envs]))
            res = [c.recv() for c in pipes]
            obs = np.concatenate([r[0] for r in res])
            B_r[t] = np.concatenate([r[1] for r in res]); B_d[t] = np.concatenate([r[2] for r in res])
            for w, r in enumerate(res):
                for k, x in enumerate(r[3]):
                    fins.append(x + (bool(det[w * a.envs + r[4][k]]),))
        with torch.no_grad():
            B_v[T] = net.v(torch.as_tensor(norm(obs), dtype=torch.float32)).squeeze(-1).numpy()
        adv = np.zeros((T, N), np.float32); last = 0
        for t in reversed(range(T)):
            nonterm = 1.0 - B_d[t]
            delta = B_r[t] + gamma * B_v[t + 1] * nonterm - B_v[t]
            last = delta + gamma * lam * nonterm * last
            adv[t] = last
        ret = adv + B_v[:T]
        o = torch.as_tensor(B_o.reshape(-1, obs_dim)); ac = torch.as_tensor(B_a.reshape(-1, act_dim))
        olp = torch.as_tensor(B_lp.reshape(-1)); A_ = torch.as_tensor(adv.reshape(-1)); R_ = torch.as_tensor(ret.reshape(-1))
        A_ = (A_ - A_.mean()) / (A_.std() + 1e-8)
        M_ = torch.as_tensor(np.broadcast_to(~det, (T, N)).reshape(-1).astype(np.float32))
        n = o.shape[0]; mb = n // n_mb; kl_mean = 0
        for ep in range(epochs):
            perm = torch.randperm(n)
            for k in range(n_mb):
                idx = perm[k * mb:(k + 1) * mb]
                dist = net.dist(o[idx]); lp = dist.log_prob(ac[idx]).sum(-1)
                ratio = (lp - olp[idx]).exp()
                mk = M_[idx]; nm = mk.sum().clamp(min=1.0)
                l_pi = -(torch.min(ratio * A_[idx], ratio.clamp(1 - clip, 1 + clip) * A_[idx]) * mk).sum() / nm
                l_v = (((net.v(o[idx]).squeeze(-1) - R_[idx]) ** 2) * mk).sum() / nm
                ent = dist.entropy().sum(-1).mean()
                loss = l_pi + 0.5 * l_v - 0.0 * ent
                opt.zero_grad(); loss.backward(); torch.nn.utils.clip_grad_norm_(net.parameters(), 1.0); opt.step()
                with torch.no_grad():
                    kl_mean = float(((olp[idx] - lp) * mk).sum() / nm)
            # adaptive learning rate on the approximate KL
            for g in opt.param_groups:
                if kl_mean > 0.02: g['lr'] = max(1e-5, g['lr'] / 1.5)
                elif kl_mean < 0.005: g['lr'] = min(1e-3, g['lr'] * 1.5)
        total += T * N; it += 1
        recent = (recent + [x for x in fins if not x[5]])[-400:]
        det_win = det_win + [x[2] for x in fins if x[5]]
        if recent:
            rets = np.mean([x[0] for x in recent]); succ = np.mean([x[2] for x in recent])
            sp = [x[2] for x in recent if x[3] == 'push']; sf = [x[2] for x in recent if x[3] == 'fallen']
            lens = np.mean([x[1] for x in recent])
        else:
            rets = succ = lens = float('nan'); sp = sf = []
        dwin = det_win[-60:]
        succ_det = float(np.mean(dwin)) if dwin else float('nan')
        row = [it, round(active), total, round(float(rets), 2), round(float(lens), 1), round(float(succ), 3),
               round(float(np.mean(sp)), 3) if sp else '', round(float(np.mean(sf)), 3) if sf else '',
               round(float(net.log_std.exp().mean()), 3), round(kl_mean, 4), opt.param_groups[0]['lr']]
        # curriculum: widen the starts only when the DETERMINISTIC policy
        # succeeds (>= 80 % of >= 24 episodes at this level)
        if a.curriculum and len(dwin) >= 24 and succ_det >= 0.8 and severity < 1.0:
            severity = round(min(1.0, severity + 0.1), 2)
            for c in pipes:
                c.send(('level', severity))
            for c in pipes:
                c.recv()
            det_win = []; recent = []
        row += [round(severity, 2), round(succ_det, 3) if dwin else '', len(dwin)]
        wr.writerow(row); f.flush()
        if it % 10 == 0:
            print(*row, flush=True)
            torch.save(dict(net=net.state_dict(), norm=norm.__dict__, meta=meta), os.path.join(a.out, 'policy.pt'))
            export(net, norm, os.path.join(a.out, 'policy.json'), meta)
            score = severity + (succ_det if dwin and len(dwin) >= 24 else 0.0)
            if score >= best:
                best = score
                torch.save(dict(net=net.state_dict(), norm=norm.__dict__, meta=meta), os.path.join(a.out, 'policy_best.pt'))
                export(net, norm, os.path.join(a.out, 'policy_best.json'), meta)
        if a.hard_stop_after and it >= a.hard_stop_after:
            print('TEST hard stop at', it, flush=True)
            os._exit(9)
        if (a.ckpt_iters and it % a.ckpt_iters == 0) or time.time() - t_ck >= a.ckpt_min * 60:
            checkpoint(); t_ck = time.time()
        if a.deadline and time.time() >= a.deadline:
            checkpoint()
            print('deadline: checkpointed at iter', it, flush=True)
            for c in pipes:
                c.send(('close', None))
            sys.exit(75)
    torch.save(dict(net=net.state_dict(), norm=norm.__dict__, meta=meta), os.path.join(a.out, 'policy.pt'))
    export(net, norm, os.path.join(a.out, 'policy.json'), meta)
    checkpoint()
    open(os.path.join(a.out, 'TRAINED'), 'w').write(json.dumps(dict(it=it, steps=total, active_h=active / 3600)))
    for c in pipes:
        c.send(('close', None))
    print('done', it, 'iterations', total, 'steps', flush=True)


if __name__ == '__main__':
    main()
