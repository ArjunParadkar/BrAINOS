"""Gate E2 training: PPO on the generated hexapod, from the self-model alone.

    # laptop smoke (one process, under a minute)
    $MOTOR_PY motor/gait/train.py --out motor/gait/runs/smoke --seconds 40 \
        --envs 16 --eval-every 5 --eval-seeds 4 --eval-seconds 5
    # cluster (see motor/hpcc/e2_train.sbatch)
    $MOTOR_PY motor/gait/train.py --out RUN --env-steps 200000000 --hours 20 \
        --envs 256 --workers 32 --threads 4

Writes into --out:
    config.json        every argument, versions, model sha256, git commit
    log.csv            one row per PPO iteration (wall, CPU time, env steps, returns, losses,
                       and the stable-gait eval when one ran)
    ckpt_latest.pt     resumable (agent, optimiser, counters)
    ckpt_NNNNN.pt      every evaluated iteration
    first_stable.pt    the first checkpoint whose float policy met the stable-gait criterion
    first_stable.json  its iteration, env steps, wall-clock and CPU time
    summary.json       at the end

"Stable gait" here is eval.stable_gait run on the deterministic float
actor at --eval-seeds x --eval-seconds (seeds 1000+, disjoint from the
final eval's). The gate's verdict is eval.py on the exported int8 blob.

Parallelism is off unless asked: --workers 1 steps every environment in
this process, and --threads 1 keeps torch on one thread.
"""
import argparse, csv, json, os, platform, subprocess, sys, time
import numpy as np
import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import env as E
import eval as EV
from ppo import Agent, Buffer, float_policy, gae, update

COLS = ['iter', 'env_steps', 'wall_s', 'cpu_s', 'gpu_s', 'sps', 'ep_return', 'ep_len_s', 'ep_speed_mps',
        'ep_fell', 'episodes', 'pi_loss', 'vf_loss', 'kl', 'clipfrac', 'std', 'lr',
        'eval_speed_mps', 'eval_upright', 'eval_n', 'eval_pass']


def git_commit():
    try:
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        c = subprocess.run(['git', '-C', root, 'rev-parse', 'HEAD'], capture_output=True, text=True).stdout.strip()
        dirty = subprocess.run(['git', '-C', root, 'status', '--porcelain', '--', 'motor'],
                               capture_output=True, text=True).stdout.strip()
        return c + (' (motor/ has uncommitted changes)' if dirty else '')
    except Exception:
        return 'unknown'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True)
    ap.add_argument('--seed', type=int, default=0)
    ap.add_argument('--envs', type=int, default=16)
    ap.add_argument('--rollout', type=int, default=64, help='control steps per env per iteration')
    ap.add_argument('--env-steps', type=float, default=float('inf'), help='budget, control steps')
    ap.add_argument('--seconds', type=float, default=float('inf'), help='budget, wall seconds')
    ap.add_argument('--hours', type=float, default=None, help='budget, wall hours (overrides --seconds)')
    ap.add_argument('--max-ep-s', type=float, default=10.0)
    ap.add_argument('--no-dr', action='store_true')
    ap.add_argument('--contact-tc', type=float, default=None,
                    help='train (and run periodic evals) with every contact time constant set to this, s; '
                         'default: the model as generated (0.02 s). See README: soft-contact exploit')
    ap.add_argument('--lr', type=float, default=3e-4)
    ap.add_argument('--epochs', type=int, default=5)
    ap.add_argument('--minibatches', type=int, default=4)
    ap.add_argument('--gamma', type=float, default=0.99)
    ap.add_argument('--lam', type=float, default=0.95)
    ap.add_argument('--hidden', default='64,64')
    ap.add_argument('--eval-every', type=int, default=20, help='iterations')
    ap.add_argument('--eval-seeds', type=int, default=20)
    ap.add_argument('--eval-seconds', type=float, default=10.0)
    ap.add_argument('--stop-when-stable', action='store_true')
    ap.add_argument('--workers', type=int, default=1, help='env worker processes (cluster only)')
    ap.add_argument('--threads', type=int, default=1, help='torch threads')
    ap.add_argument('--device', default='cpu')
    ap.add_argument('--resume', action='store_true')
    a = ap.parse_args()
    budget_s = a.hours * 3600 if a.hours is not None else a.seconds
    hidden = tuple(int(x) for x in a.hidden.split(','))

    torch.set_num_threads(a.threads)
    torch.manual_seed(a.seed)
    np.random.seed(a.seed)
    dev = torch.device(a.device)
    os.makedirs(a.out, exist_ok=True)
    cfg = dict(vars(a), hidden=list(hidden), budget_s=budget_s, model=E.MODEL_PATH,
               model_sha256=E.model_sha256(), git=git_commit(), host=platform.node(),
               python=sys.version.split()[0], torch=torch.__version__, numpy=np.__version__,
               mujoco=__import__('mujoco').__version__, obs_range=E.OBS_RANGE.tolist(),
               action_scale=E.ACTION_SCALE,
               reward=dict(vel=E.W_VEL, lat=E.W_LAT, yaw=E.W_YAW, vz=E.W_VZ, rock=E.W_RP, tilt=E.W_TILT,
                           alive=E.W_ALIVE, limit=E.W_LIMIT, effort=E.W_EFFORT),
               servo_rate_dps=E.assumed_rates_dps(),
               dr=None if a.no_dr else dict(mass=E.DR_MASS, friction=E.DR_FRIC, kp=E.DR_KP),
               started=time.strftime('%Y-%m-%dT%H:%M:%S%z'))

    agent = Agent(hidden).to(dev)
    opt = torch.optim.Adam(agent.parameters(), lr=a.lr, eps=1e-5)
    it, env_steps, wall0, cpu0, first_stable = 0, 0, 0.0, 0.0, None
    latest = os.path.join(a.out, 'ckpt_latest.pt')
    if a.resume and os.path.exists(latest):
        ck = torch.load(latest, map_location=dev, weights_only=False)
        agent.load_state_dict(ck['agent'])
        opt.load_state_dict(ck['opt'])
        it, env_steps, wall0, cpu0 = ck['iter'], ck['env_steps'], ck['wall_s'], ck['cpu_s']
        first_stable = ck.get('first_stable')
        torch.set_rng_state(ck['torch_rng'])
        print(f'resumed at iter {it}, {env_steps} env steps')
    with open(os.path.join(a.out, 'config.json' if not a.resume else f'config_resume_{it}.json'), 'w') as f:
        json.dump(cfg, f, indent=1)

    max_steps = int(a.max_ep_s * E.CONTROL_HZ)
    seed = a.seed * 1_000_003 + it          # a resumed run gets fresh env streams
    venv = (E.ProcVecEnv(a.envs, seed, not a.no_dr, max_steps, a.workers, a.contact_tc) if a.workers > 1
            else E.VecEnv(a.envs, seed, not a.no_dr, max_steps, a.contact_tc))
    obs = torch.as_tensor(venv.reset(), device=dev)
    T, N = a.rollout, a.envs
    buf = Buffer(T, N, dev)
    t_start, c_start = time.time(), time.process_time()
    ep_ret = np.zeros(N)
    log_path = os.path.join(a.out, 'log.csv')
    new_log = not os.path.exists(log_path)
    logf = open(log_path, 'a', newline='')
    log = csv.DictWriter(logf, fieldnames=COLS)
    if new_log:
        log.writeheader()

    def clocks():
        wall = wall0 + time.time() - t_start
        cpu = cpu0 + time.process_time() - c_start + venv.cpu_seconds()
        return wall, cpu, (wall if dev.type == 'cuda' else 0.0)

    def save(path, extra=None):
        w, c, _ = clocks()
        torch.save(dict(agent=agent.state_dict(), opt=opt.state_dict(), iter=it, env_steps=env_steps,
                        wall_s=w, cpu_s=c, first_stable=first_stable, config=cfg,
                        torch_rng=torch.get_rng_state(), **(extra or {})), path)

    finished, done_ret, done_len, done_prog, done_fell = [], [], [], [], []
    # the wall budget is per invocation, so a resumed job gets its own
    while env_steps < a.env_steps and time.time() - t_start < budget_s:
        it_t0 = time.time()
        for t in range(T):
            a_t, logp, v = agent.act(obs)
            o2, r, te, tr, final, stats = venv.step(a_t.cpu().numpy())
            buf.obs[t], buf.act[t], buf.logp[t], buf.val[t] = obs, a_t, logp, v
            buf.rew[t] = torch.as_tensor(r, dtype=torch.float32, device=dev)
            buf.term[t] = torch.as_tensor(te, dtype=torch.float32, device=dev)
            buf.trunc[t] = torch.as_tensor(tr, dtype=torch.float32, device=dev)
            if tr.any():
                with torch.no_grad():
                    fv = agent.critic(torch.as_tensor(final, device=dev))
                buf.boot[t] = fv * buf.trunc[t]
            else:
                buf.boot[t] = 0.0
            ep_ret += r
            for i in np.flatnonzero(te | tr):
                done_ret.append(ep_ret[i])
                ep_ret[i] = 0.0
            for (length, prog, fell) in stats:
                done_len.append(length / E.CONTROL_HZ)
                done_prog.append(prog / max(length / E.CONTROL_HZ, 1e-9))
                done_fell.append(fell)
            obs = torch.as_tensor(o2, device=dev)
        env_steps += T * N
        with torch.no_grad():
            last_val = agent.critic(obs)
        adv, ret = gae(buf, last_val, a.gamma, a.lam)
        st = update(agent, opt, buf, adv, ret, a.epochs, a.minibatches)
        it += 1
        wall, cpu, gpu = clocks()
        row = dict(iter=it, env_steps=env_steps, wall_s=round(wall, 2), cpu_s=round(cpu, 2), gpu_s=round(gpu, 2),
                   sps=round(T * N / max(time.time() - it_t0, 1e-9), 1),
                   ep_return=np.mean(done_ret) if done_ret else '', ep_len_s=np.mean(done_len) if done_len else '',
                   ep_speed_mps=np.mean(done_prog) if done_prog else '',
                   ep_fell=np.mean(done_fell) if done_fell else '', episodes=len(done_len),
                   pi_loss=st['pi'], vf_loss=st['vf'], kl=st['kl'], clipfrac=st['clipfrac'],
                   std=float(agent.log_std.detach().exp().mean()), lr=a.lr)
        done_ret, done_len, done_prog, done_fell = [], [], [], []
        if it % a.eval_every == 0:
            pol = EV.Learned('float', float_policy(agent.actor.cpu()))
            agent.actor.to(dev)
            ev = EV.stable_gait(pol, list(range(1000, 1000 + a.eval_seeds)), a.eval_seconds,
                                contact_tc=a.contact_tc)
            row.update(eval_speed_mps=ev['mean_speed_mps'], eval_upright=ev['upright'], eval_n=ev['n'],
                       eval_pass=int(ev['passes']))
            wall, cpu, gpu = clocks()          # eval time is part of the clock
            if ev['passes'] and first_stable is None:
                first_stable = dict(iter=it, env_steps=env_steps, wall_s=wall, cpu_s=cpu, gpu_s=gpu,
                                    device=a.device, eval={k: v for k, v in ev.items() if k != 'episodes'})
                # not pre-registered, recorded beside it: the same test on a 2x finer physics step
                seeds = list(range(1000, 1000 + a.eval_seeds))
                dc = EV.stable_gait(pol, seeds, a.eval_seconds, phys_div=2, contact_tc=a.contact_tc)
                first_stable['dt_check_not_preregistered'] = {k: v for k, v in dc.items() if k != 'episodes'}
                if a.contact_tc is None:
                    cc = EV.stable_gait(pol, seeds, a.eval_seconds, contact_tc=0.005)
                    first_stable['contact_check_not_preregistered'] = {k: v for k, v in cc.items()
                                                                       if k != 'episodes'}
                save(os.path.join(a.out, 'first_stable.pt'))
                with open(os.path.join(a.out, 'first_stable.json'), 'w') as f:
                    json.dump(first_stable, f, indent=1)
                print(f'FIRST STABLE at iter {it}: {env_steps} env steps, {wall:.1f} s wall, {cpu:.1f} s CPU')
            save(os.path.join(a.out, f'ckpt_{it:05d}.pt'), dict(eval={k: v for k, v in ev.items() if k != 'episodes'}))
            save(latest)
        log.writerow(row)
        logf.flush()
        print(' '.join(f'{k}={v:.4g}' if isinstance(v, float) else f'{k}={v}' for k, v in row.items()
                       if k in ('iter', 'env_steps', 'wall_s', 'ep_return', 'ep_speed_mps', 'ep_fell',
                                'std', 'eval_speed_mps', 'eval_upright')), flush=True)
        if a.stop_when_stable and first_stable is not None:
            break
    save(latest)
    torch.save(dict(agent=agent.state_dict(), config=cfg, iter=it, env_steps=env_steps), os.path.join(a.out, 'final.pt'))
    wall, cpu, gpu = clocks()
    summary = dict(iters=it, env_steps=env_steps, physics_steps=env_steps * E.PHYS_PER_CTRL, wall_s=wall,
                   cpu_s=cpu, gpu_s=gpu, device=a.device, first_stable=first_stable)
    with open(os.path.join(a.out, 'summary.json'), 'w') as f:
        json.dump(summary, f, indent=1)
    print(json.dumps(summary))
    venv.close()
    logf.close()


if __name__ == '__main__':
    main()
