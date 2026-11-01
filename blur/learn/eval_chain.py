#!/usr/bin/env python3
"""
eval_chain.py -- the Gate 2 reflex chain, evaluated the way the supervisor
runs it (os/blur/src/supervisor.rs: same states, clocks and predicates),
on the training physics with domain randomisation, deterministic policies.

  fallen : 200 starts from the get-up task's fallen-pose generator (every
           attitude, random joints, dropped and settled). Success = the stand
           predicate (upright, at height, near the stand pose, still) held for
           2 s inside 12 s.
  push   : standing in the ready stance, one 10-60 N x 100 ms push in a
           random direction. Success = never fell and back standing still
           for 2 s, OR fell and the chain got it up -- inside 12 s (Gate 2's
           rule).

  python eval_chain.py --mode fallen --episodes 200 [--righting P --standup P --balance P]
"""
import argparse, json, math, multiprocessing as mp, os, sys
os.environ.setdefault('OMP_NUM_THREADS', '1')
import numpy as np
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)

BRACE_MAX, BRACE_STILL = 1.5, 0.2
SKILL_TIMEOUT, STERNAL_HOLD, STAND_HOLD, TRIES, BLEND = 6.0, 0.5, 2.0, 3, 0.6
FALL_TILT, FALL_TILT_FAST, FALL_RATE, FALL_CONFIRM = 50.0, 30.0, 2.5, 0.03


class Stub:
    action_mode = 'range'

    def __init__(self):
        import env as E
        self.nom = E.QSTAND; self.cmd = np.zeros(1)

    def nominal(self, env):
        return self.nom

    def command(self, env):
        return self.cmd

    def gains(self, env):
        import env as E
        return E.stand_kp()


def run(args):
    mode, seeds, paths = args
    import env as E, tasks as T, skills as S
    from eval import load_policy
    from ref.keyframes import STERNAL
    pol = {k: load_policy(p)[1] for k, p in paths.items()}
    RREF = S.load_righting()

    def filt(targets, new, alpha):
        return targets + alpha * (new - targets)
    PJ = np.asarray(E.POLICY_JOINTS)
    out = []
    for seed in seeds:
        stub = Stub()
        e = E.BlurEnv(stub, seed=seed)
        e.randomize_body(); kp, kd = E.stand_kp(); e.set_gains(kp * e.strength, kd)
        e.push = None; e.prev_action = np.zeros(E.NA); e.track_attitude = False; e.att = None
        if mode == 'fallen':
            g = T.GetUp(p_push=0.0, severity=1.0); g.reset(e)
            state, t0 = 'brace', 0.0
        else:
            b = S.Balance(level=1.0); b.start(e)
            state, t0 = 'hold', -10.0
        e.t = 0.0; e.att = None; e.track_attitude = True
        targets = e.d.ctrl[e.act].copy()
        pred = still = 0.0; tries = 0; fell = mode == 'fallen'; fall_t = 0.0; t_ok = None
        held_stand = 0.0; hold_from = targets.copy(); hold_t0 = 0.0
        while e.t < 12.0:
            if e.att is None:
                e.att_update()
            up = e.up(); tilt = math.degrees(math.acos(max(-1.0, min(1.0, up))))
            rate = float(np.linalg.norm(e.att.last_w))
            q, dq = e.q(), e.dq()
            def near(pose, tol):
                return bool(np.all(np.abs((q - pose)[PJ]) < tol) and np.all(np.abs(dq[PJ]) < 1.5))
            # fall detection (from standing)
            if state == 'hold':
                falling = tilt > FALL_TILT or (tilt > FALL_TILT_FAST and rate > FALL_RATE)
                fall_t = fall_t + 0.02 if falling else 0.0
                if fall_t >= FALL_CONFIRM:
                    fell = True; state, t0, tries, pred, still = 'brace', e.t, 0, 0.0, 0.0
                    e.prev_action = np.zeros(E.NA)
            el = e.t - t0
            braced = state == 'brace'
            if braced != getattr(e, '_soft', False):           # the supervisor braces at half stiffness
                e.set_gains(kp * e.strength * (0.5 if braced else 1.0), kd * (0.6 if braced else 1.0)); e._soft = braced
            if state == 'brace':
                still = still + 0.02 if rate < 0.5 else 0.0
                targets = S.STERNAL if hasattr(S, 'STERNAL') else STERNAL
                if still >= BRACE_STILL or el >= BRACE_MAX:
                    # the supervisor's rule, on the attitude estimate
                    g = np.array(e.att.gravity_body()); upg = -g[2]
                    state = 'standup' if upg > 0.7 else 'righting'; t0 = e.t; pred = 0.0; e.prev_action = np.zeros(E.NA)
                    if state == 'righting':
                        cls = 'back' if upg < -0.7 else ('right' if math.atan2(-g[1], -g[2]) > 0 else 'left')
                        ref = RREF[cls]
                    else:
                        ref = S.STANDUP_REF
                    targets = ref(0.0)
            elif state in ('righting', 'standup'):
                stub.nom = ref(el); stub.cmd = np.array([min(el / 6.0, 1.0)])
                a = pol[state](e.observe(stub.cmd))
                targets = filt(targets, E.targets_from_action(stub.nom, a, 'offset'), 0.1); e.prev_action = np.clip(a, -1, 1)
                if state == 'righting':
                    ok = tilt < 25.8 and rate < 0.5 and near(STERNAL, 0.5)
                    pred = pred + 0.02 if ok else 0.0
                    if pred >= STERNAL_HOLD:
                        state, t0, pred = 'standup', e.t, 0.0; e.prev_action = np.zeros(E.NA)
                        ref = S.STANDUP_REF; targets = ref(0.0)
                else:
                    ok = tilt < 19.9 and rate < 0.5 and near(E.QSTAND, 0.35)
                    pred = pred + 0.02 if ok else 0.0
                    if pred >= STAND_HOLD:
                        state, hold_t0, hold_from = 'hold', e.t, targets.copy(); e.prev_action = np.zeros(E.NA)
                if state in ('righting', 'standup') and e.t - t0 > SKILL_TIMEOUT:
                    tries += 1
                    if tries >= TRIES:
                        break
                    state, t0, still = 'brace', e.t, 0.0
            elif state == 'hold':
                he = e.t - hold_t0
                if he < BLEND and mode == 'fallen':
                    s = min(max(he / BLEND, 0), 1); s = s ** 3 * (10 - 15 * s + 6 * s * s)
                    targets = hold_from + s * (S.READY - hold_from)
                else:
                    stub.nom = S.READY; stub.cmd = np.zeros(1)
                    a = pol['balance'](e.observe(stub.cmd))
                    targets = filt(targets, E.targets_from_action(S.READY, a, 'offset'), 0.3); e.prev_action = np.clip(a, -1, 1)
            # the success the gate counts: standing, still, for 2 s
            standing = state == 'hold' and up > 0.94 and float(e.d.qpos[2]) > 0.2 and rate < 0.5
            held_stand = held_stand + 0.02 if standing else 0.0
            if held_stand >= 2.0 and e.t > 0.2:
                t_ok = e.t
                break
            old = e.d.ctrl[e.act].copy(); lat = min(e.latency_ms, 19)
            e.apply_targets(old, lat); e.apply_targets(np.clip(targets, E.QMIN, E.QMAX), 20 - lat)
        out.append(dict(seed=int(seed), success=t_ok is not None, t=t_ok, fell=bool(fell), end_state=state,
                        force=float(np.linalg.norm(e.push[1])) if mode == 'push' and e.push else None))
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--mode', default='fallen'); p.add_argument('--episodes', type=int, default=200)
    p.add_argument('--procs', type=int, default=16)
    for k in ('righting', 'standup', 'balance'):
        p.add_argument('--' + k, default=os.path.join(HERE, f'runs/{k}_v2/policy_best.json'))
    p.add_argument('--out', default=None)
    a = p.parse_args()
    paths = dict(righting=a.righting, standup=a.standup, balance=a.balance)
    seeds = np.arange(a.episodes) + (700000 if a.mode == 'fallen' else 800000)
    chunks = [(a.mode, list(c), paths) for c in np.array_split(seeds, a.procs) if len(c)]
    with mp.get_context('fork').Pool(len(chunks)) as pool:
        res = [r for rr in pool.map(run, chunks) for r in rr]
    succ = np.mean([r['success'] for r in res])
    ts = [r['t'] for r in res if r['success']]
    summ = dict(mode=a.mode, episodes=len(res), success=float(succ), fell=int(sum(r['fell'] for r in res)),
                t_median=float(np.median(ts)) if ts else None, t_p90=float(np.percentile(ts, 90)) if ts else None,
                end_states={s: sum(r['end_state'] == s for r in res) for s in set(r['end_state'] for r in res)},
                policies=paths)
    print(json.dumps(summ, indent=1))
    if a.out:
        json.dump(dict(summary=summ, episodes=res), open(a.out, 'w'), indent=1)


if __name__ == '__main__':
    main()
