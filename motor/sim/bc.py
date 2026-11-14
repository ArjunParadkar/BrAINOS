"""The baseline: behaviour cloning from demonstrations.

A small network regresses the demonstrated motion from what the recipe
also gets to see: where the tool is relative to each named place, the
fingers, what is felt, the mug's radius and depth, and how far through
the task it is. It is run closed loop through the same inverse
kinematics and joint loop the demonstration used (`ArmSim.step_teleop`).
It does not go through the seat: there is no primitive to ask the gate
for, which is part of what the comparison shows.
"""
import math
import numpy as np
import torch
import arm_sim as A
import mug_task as M

DT_MS = 10          # the demonstration is logged, and the clone decides, at 100 Hz
POS_SCALE = 0.004   # metres of tool motion per unit of network output


def read_demo(path):
    """[(sample, {place name: (pos, attrs)})] from a demonstration file."""
    out = []
    for line in open(path):
        f = line.split()
        if f[0] == 'd':
            out.append((dict(t=float(f[1]), tool=np.array([float(x) for x in f[2:5]]), open=float(f[5]),
                             grip=float(f[7]), touch=float(f[8])), {}))
        elif f[0] == 'pl':
            out[-1][1][f[1]] = (np.array([float(x) for x in f[2:5]]), dict(kv.split('=') for kv in f[6:]))
    return out


def observe(tool, opening, grip_n, touch_n, places, frac):
    def rel(name):
        return (tool - places[name][0]) if name in places else np.zeros(3)
    mouth = places.get('mug.mouth', (None, {}))[1]
    # the brush's grip point is where the brush is when it stands, and
    # where the hand is once it is held
    return np.concatenate([
        rel('brush.grip') * 10, rel('mug.mouth') * 10, rel('holder.rest') * 10,
        [opening * 50, float(grip_n > 0.8), float(touch_n > 0.6),
         float(mouth.get('radius', 0)) * 20, float(mouth.get('depth', 0)) * 20, frac]]).astype(np.float32)


def dataset(paths):
    X, Y = [], []
    for path in paths:
        demo = read_demo(path)
        T = demo[-1][0]['t'] - demo[0][0]['t']
        last_places = {}
        for i in range(len(demo) - 1):
            s, places = demo[i]
            last_places.update(places)
            nxt = demo[i + 1][0]
            frac = (s['t'] - demo[0][0]['t']) / T
            X.append(observe(s['tool'], s['open'], s['grip'], s['touch'], last_places, frac))
            Y.append(np.concatenate([(nxt['tool'] - s['tool']) / POS_SCALE, [nxt['open'] * 50]]).astype(np.float32))
    return np.array(X), np.array(Y)


def train(paths, epochs=1500, seed=0):
    torch.manual_seed(seed)
    torch.set_num_threads(1)
    X, Y = dataset(paths)
    x, y = torch.tensor(X), torch.tensor(Y)
    net = torch.nn.Sequential(torch.nn.Linear(X.shape[1], 128), torch.nn.ReLU(),
                              torch.nn.Linear(128, 128), torch.nn.ReLU(), torch.nn.Linear(128, 4))
    opt = torch.optim.Adam(net.parameters(), lr=1e-3)
    # the same number of updates whatever the number of demonstrations;
    # above 8,192 samples each update sees a random 8,192 of them
    batch = 8192
    for _ in range(epochs):
        opt.zero_grad()
        if len(x) > batch:
            i = torch.randint(0, len(x), (batch,))
            loss = torch.nn.functional.mse_loss(net(x[i]), y[i])
        else:
            loss = torch.nn.functional.mse_loss(net(x), y)
        loss.backward()
        opt.step()
    return net, float(loss.detach()), len(X)


def rollout(net, c, demo_seconds, seed=0, slack=1.3):
    """Run the clone on one instance. Returns the judge's verdict."""
    sim = M.scene(c, seed=seed, teleop=True)
    for _ in range(200):
        sim.step_teleop(A.Q0)
    steps = int(demo_seconds * 1000 * slack / DT_MS)
    target = sim.tool_pos().copy()
    opening = A.Q0[5]
    q = list(A.Q0)
    stuck = False
    for k in range(steps):
        places = {name: (np.array(pos), attrs) for name, pos, yaw, attrs in sim.places_fn(sim)}
        frac = min(1.0, k * DT_MS / (demo_seconds * 1000))
        obs = observe(sim.tool_pos(), sim.d.qpos[sim.qadr[5]], sim.last['grip_n'], sim.last['tool_n'], places, frac)
        with torch.no_grad():
            a = net(torch.tensor(obs)[None])[0].numpy()
        # the network's output is the tool's next displacement; it is
        # added to the commanded position, not to the measured one, or
        # the joint loop's lag would be subtracted from every step and
        # the arm would hardly move
        target = target + np.clip(a[:3], -3, 3) * POS_SCALE
        opening = float(np.clip(a[3] / 50, 0.0, 0.045))
        sol = A.solve(target, -90.0, 0.0, opening)
        if sol is None:
            stuck = True      # asked for somewhere the arm cannot reach: hold the last pose
        else:
            q = sol
        for _ in range(DT_MS):
            sim.step_teleop(q)
    for _ in range(600):
        sim.step_teleop(q)
    v = M.judge(sim)
    v['unreachable'] = stuck
    sim.close()
    return v
