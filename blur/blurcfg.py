"""
blurcfg.py -- where things are on this machine (laptop or cluster).

Every BLUR script loads the body descriptor through `load_descriptor()`.
The descriptor stays the source of truth for the body; only the location
of the MJCF on disk is a per-machine setting:

  BLUR_MJCF_DIR   directory holding blur.xml, scene_*.xml, moves.npz, meshes/
                  (default: descriptor.json's `mjcf_dir`, the laptop path)

Wherever the MJCF comes from, blur.xml must hash to the descriptor's
`mjcf_sha256`; a stale or edited copy is refused, so a cluster run can
never silently train on a different body. BLUR_ALLOW_MJCF_MISMATCH=1
turns the refusal into a warning (experiments on a modified copy only).

The cluster side sets these in blur/hpcc/site/pinnacle.env; the laptop
needs nothing.
"""
import hashlib, json, os, sys

ROOT = os.path.dirname(os.path.abspath(__file__))
# BLUR_DESCRIPTOR overrides which descriptor is the body (default: v36's
# descriptor.json). The final-body retarget campaign points this at
# descriptor_final.json so it trains on BLUR_final without editing the
# committed v36 descriptor the day-one campaign is pinned to.
DESCRIPTOR = os.environ.get('BLUR_DESCRIPTOR') or os.path.join(ROOT, 'descriptor.json')
_cache = {}


def mjcf_dir(desc):
    return os.environ.get('BLUR_MJCF_DIR') or desc['mjcf_dir']


def _check(d, want):
    xml = os.path.join(d, 'blur.xml')
    if not os.path.exists(xml):
        sys.exit(f"blurcfg: no blur.xml in {d} (set BLUR_MJCF_DIR)")
    got = hashlib.sha256(open(xml, 'rb').read()).hexdigest()
    if got != want:
        msg = f"blurcfg: {xml} sha256 {got[:12]} != descriptor {want[:12]}"
        if os.environ.get('BLUR_ALLOW_MJCF_MISMATCH') == '1':
            print('WARNING ' + msg, file=sys.stderr)
        else:
            sys.exit(msg + ' (wrong or edited MJCF copy)')


def load_descriptor():
    """descriptor.json with `mjcf_dir` resolved for this machine (checked once per process)"""
    if 'desc' not in _cache:
        desc = json.load(open(DESCRIPTOR))
        desc['mjcf_dir'] = mjcf_dir(desc)
        _check(desc['mjcf_dir'], desc['mjcf_sha256'])
        _cache['desc'] = desc
    return _cache['desc']


def mjcf(name):
    """path of a file in the MJCF directory (scene_flat.xml, moves.npz, ...)"""
    return os.path.join(load_descriptor()['mjcf_dir'], name)
