"""A small PPO for the E2 hexapod (torch, CPU by default).

The actor is the network that ships: 36 -> 64 -> 64 -> 10, ReLU hidden
layers, linear output (the action mean). That is the only shape the
int8 runner in `os/motor/src/policy.rs` runs (ReLU hidden layers, a
clamped linear last layer), so nothing here may add a tanh or a
normaliser with an offset. Input scaling is fixed and physical
(`env.OBS_RANGE`), not learned and not a running mean.

The critic is training-only and does not ship.
"""
import numpy as np
import torch
import torch.nn as nn

from env import N_ACT, N_OBS, OBS_RANGE


class Actor(nn.Module):
    def __init__(self, hidden=(64, 64)):
        super().__init__()
        self.register_buffer('obs_range', torch.as_tensor(OBS_RANGE, dtype=torch.float32))
        layers, w = [], N_OBS
        for h in hidden:
            layers += [nn.Linear(w, h), nn.ReLU()]
            w = h
        layers.append(nn.Linear(w, N_ACT))
        self.net = nn.Sequential(*layers)
        # small last layer: start near the stand pose
        nn.init.orthogonal_(self.net[-1].weight, gain=0.01)
        nn.init.zeros_(self.net[-1].bias)
        self.hidden = tuple(hidden)

    def scaled(self, obs):
        return torch.clamp(obs / self.obs_range, -1.0, 1.0)

    def forward(self, obs):
        return self.net(self.scaled(obs))

    def linears(self):
        return [m for m in self.net if isinstance(m, nn.Linear)]


class Critic(nn.Module):
    def __init__(self, hidden=(128, 128)):
        super().__init__()
        self.register_buffer('obs_range', torch.as_tensor(OBS_RANGE, dtype=torch.float32))
        layers, w = [], N_OBS
        for h in hidden:
            layers += [nn.Linear(w, h), nn.Tanh()]
            w = h
        layers.append(nn.Linear(w, 1))
        self.net = nn.Sequential(*layers)

    def forward(self, obs):
        return self.net(torch.clamp(obs / self.obs_range, -1.0, 1.0)).squeeze(-1)


class Agent(nn.Module):
    def __init__(self, hidden=(64, 64), log_std0=-0.7):
        super().__init__()
        self.actor = Actor(hidden)
        self.critic = Critic()
        self.log_std = nn.Parameter(torch.full((N_ACT,), float(log_std0)))

    def dist(self, obs):
        mu = self.actor(obs)
        return torch.distributions.Normal(mu, self.log_std.exp().expand_as(mu))

    @torch.no_grad()
    def act(self, obs):
        d = self.dist(obs)
        a = d.sample()
        return a, d.log_prob(a).sum(-1), self.critic(obs)

    @torch.no_grad()
    def mean_action(self, obs):
        return torch.clamp(self.actor(obs), -1.0, 1.0)


class Buffer:
    def __init__(self, T, N, device):
        z = lambda *s: torch.zeros(*s, device=device)
        self.obs, self.act = z(T, N, N_OBS), z(T, N, N_ACT)
        self.logp, self.rew, self.val = z(T, N), z(T, N), z(T, N)
        self.term, self.trunc, self.boot = z(T, N), z(T, N), z(T, N)
        self.T, self.N = T, N


def gae(buf, last_val, gamma, lam):
    """Advantages with time-limit bootstrapping: a truncated step gets
    gamma * V(final obs) added to its reward, then is treated as an end."""
    T = buf.T
    adv = torch.zeros_like(buf.rew)
    last = torch.zeros_like(last_val)
    rew = buf.rew + gamma * buf.boot * buf.trunc
    for t in reversed(range(T)):
        nxt = last_val if t == T - 1 else buf.val[t + 1]
        end = torch.clamp(buf.term[t] + buf.trunc[t], 0, 1)
        delta = rew[t] + gamma * nxt * (1 - end) - buf.val[t]
        last = delta + gamma * lam * (1 - end) * last
        adv[t] = last
    return adv, adv + buf.val


def update(agent, opt, buf, adv, ret, epochs=5, minibatches=4, clip=0.2, vf_coef=0.5,
           ent_coef=0.0, max_grad=1.0):
    n = buf.T * buf.N
    obs = buf.obs.reshape(n, -1)
    act = buf.act.reshape(n, -1)
    logp0 = buf.logp.reshape(n)
    adv = adv.reshape(n)
    ret = ret.reshape(n)
    adv = (adv - adv.mean()) / (adv.std() + 1e-8)
    mb = max(1, n // minibatches)
    stats = dict(pi=0.0, vf=0.0, kl=0.0, clipfrac=0.0)
    k = 0
    for _ in range(epochs):
        perm = torch.randperm(n, device=obs.device)
        for s in range(0, n, mb):
            i = perm[s:s + mb]
            d = agent.dist(obs[i])
            logp = d.log_prob(act[i]).sum(-1)
            ratio = (logp - logp0[i]).exp()
            pg = -torch.min(ratio * adv[i], torch.clamp(ratio, 1 - clip, 1 + clip) * adv[i]).mean()
            vf = ((agent.critic(obs[i]) - ret[i]) ** 2).mean()
            ent = d.entropy().sum(-1).mean()
            loss = pg + vf_coef * vf - ent_coef * ent
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(agent.parameters(), max_grad)
            opt.step()
            with torch.no_grad():
                stats['pi'] += float(pg)
                stats['vf'] += float(vf)
                stats['kl'] += float((logp0[i] - logp).mean())
                stats['clipfrac'] += float(((ratio - 1).abs() > clip).float().mean())
            k += 1
    return {a: b / max(k, 1) for a, b in stats.items()}


def actor_numpy(actor):
    """The actor as plain float32 arrays: [(W, b), ...], for export."""
    return [(l.weight.detach().cpu().numpy().astype(np.float32),
             l.bias.detach().cpu().numpy().astype(np.float32)) for l in actor.linears()]


def float_policy(actor):
    """obs (float32, N_OBS) -> action in [-1, 1], numpy in and out."""
    layers = actor_numpy(actor)
    rng = OBS_RANGE.astype(np.float32)

    def f(obs):
        x = np.clip(np.asarray(obs, np.float32) / rng, -1.0, 1.0)
        for k, (W, b) in enumerate(layers):
            x = W @ x + b
            if k < len(layers) - 1:
                x = np.maximum(x, 0.0)
        return np.clip(x, -1.0, 1.0)
    return f
