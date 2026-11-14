"""The Python int8 reference for `brainos_motor::policy::Int8Mlp`.

This is the arithmetic in `os/motor/src/policy.rs`, step for step, so
that the Rust runner and this file give the same integers for the same
blob and the same float32 observation:

    input   q = rint(float32(x) * float32(in_scale))   float32 multiply, ties to even
            NaN -> 0, then clip to [-127, 127]
    layer   acc = bias + sum(w * a)                     int64
            y   = (acc * mult + (1 << (shift - 1))) >> shift   arithmetic shift (floor)
            hidden: clip(y, 0, 127); last: clip(y, -127, 127)
    output  float32(y) / float32(127) * float32(out_scale)

The blob layout is in the doc comment of policy.rs. `parse` refuses
the same malformed blobs the Rust parser refuses, with the same names.

Golden sets (what the Rust test reads) are a directory of:

    blob.bin   the policy
    obs.f32    N x n_in float32, little-endian, row-major
    out.i8     N x n_out int8: the expected `act_int` result
    act.fnv    8 bytes: FNV-1a 64 of the float32 `act` outputs (N x n_out, LE bytes)
    meta.json  what produced it
"""
import json, os, struct
import numpy as np

MAGIC = b'BMOTI8\x01\x00'
MAX_WIDTH = 256


class PolicyError(ValueError):
    pass


def parse(blob):
    blob = bytes(blob)
    at = 0

    def take(n):
        nonlocal at
        if at + n > len(blob):
            raise PolicyError('Truncated')
        s = blob[at:at + n]
        at += n
        return s

    if take(8) != MAGIC:
        raise PolicyError('BadMagic')
    n_in, n_layers = struct.unpack('<HH', take(4))
    if n_in == 0 or n_in > MAX_WIDTH or n_layers == 0:
        raise PolicyError('TooWide')
    in_scale = np.frombuffer(take(4 * n_in), '<f4').astype(np.float32)
    layers, width = [], n_in
    for _ in range(n_layers):
        rows, cols, mult, shift = struct.unpack('<HHiB', take(9))
        if cols != width:
            raise PolicyError('Mismatch')
        if rows == 0 or rows > MAX_WIDTH:
            raise PolicyError('TooWide')
        if shift == 0 or shift > 40:
            raise PolicyError('BadShift')
        w = np.frombuffer(take(rows * cols), np.int8).reshape(rows, cols).astype(np.int64)
        b = np.frombuffer(take(4 * rows), '<i4').astype(np.int64)
        layers.append(dict(rows=rows, cols=cols, mult=int(mult), shift=int(shift), w=w, b=b))
        width = rows
    out_scale = np.float32(struct.unpack('<f', take(4))[0])
    if at != len(blob):
        raise PolicyError('TrailingBytes')
    return dict(in_scale=in_scale, layers=layers, out_scale=out_scale)


def load(path):
    with open(path, 'rb') as f:
        return parse(f.read())


def quantise_input(p, obs):
    x = np.asarray(obs, dtype=np.float32)
    q = np.rint(x * p['in_scale'])                 # float32 * float32 -> float32
    q = np.where(np.isnan(q), np.float32(0), q)
    return np.clip(q, -127, 127).astype(np.int64)


def act_int(p, obs):
    """obs: (n_in,) or (B, n_in) float32 -> int64 in [-127, 127]."""
    a = quantise_input(p, obs)
    last = len(p['layers']) - 1
    for k, l in enumerate(p['layers']):
        acc = a @ l['w'].T + l['b']                # exact int64
        y = (acc * l['mult'] + (1 << (l['shift'] - 1))) >> l['shift']
        a = np.clip(y, -127, 127) if k == last else np.clip(y, 0, 127)
    return a


def act(p, obs):
    q = act_int(p, obs).astype(np.float32)
    return q / np.float32(127.0) * p['out_scale']


def policy_fn(path):
    """A callable obs -> action for eval.py."""
    p = load(path)
    return lambda obs: act(p, obs).astype(np.float64)


def fnv1a64(data):
    h = 0xcbf29ce484222325
    for byte in data:
        h ^= byte
        h = (h * 0x100000001b3) & 0xffffffffffffffff
    return h


def write_golden(directory, blob, obs, meta):
    os.makedirs(directory, exist_ok=True)
    p = parse(blob)
    obs = np.ascontiguousarray(obs, dtype='<f4')
    q = act_int(p, obs)
    assert q.min() >= -127 and q.max() <= 127
    y = act(p, obs).astype('<f4')
    with open(os.path.join(directory, 'blob.bin'), 'wb') as f:
        f.write(blob)
    obs.tofile(os.path.join(directory, 'obs.f32'))
    q.astype(np.int8).tofile(os.path.join(directory, 'out.i8'))
    with open(os.path.join(directory, 'act.fnv'), 'wb') as f:
        f.write(struct.pack('<Q', fnv1a64(y.tobytes())))
    meta = dict(meta, n=int(obs.shape[0]), n_in=int(obs.shape[1]), n_out=int(q.shape[1]))
    with open(os.path.join(directory, 'meta.json'), 'w') as f:
        json.dump(meta, f, indent=1)
    return q


def read_golden(directory):
    with open(os.path.join(directory, 'meta.json')) as f:
        meta = json.load(f)
    with open(os.path.join(directory, 'blob.bin'), 'rb') as f:
        blob = f.read()
    obs = np.fromfile(os.path.join(directory, 'obs.f32'), '<f4').reshape(meta['n'], meta['n_in'])
    out = np.fromfile(os.path.join(directory, 'out.i8'), np.int8).reshape(meta['n'], meta['n_out'])
    return blob, obs, out, meta
