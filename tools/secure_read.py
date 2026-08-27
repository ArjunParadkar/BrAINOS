#!/usr/bin/env python3
"""
secure_read.py — inspect the BRAINOS-SECURE area (p3) of a key or full image.

Prints one line per TLV record (see the format note in make_key.py): the
record type, its length, and — for public records — a short preview. The
Anthropic API key (type ANTHROPIC) is a SECRET and is ALWAYS masked: this
tool never prints its contents, only that it is present and how long it is.

Usage:
  tools/secure_read.py                 # inspect ./brainos-key.img
  tools/secure_read.py path/to/img     # inspect a key image or brainos.img
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from make_key import (  # noqa: E402
    read_gpt_partitions,
    read_secure_records,
    SEC_TYPE_NAMES,
    SEC_SECRET_TYPES,
    SEC_T_BRAINKEY,
)


def find_secure(img):
    """Return the BRAINOS-SECURE payload from a GPT image, or None."""
    for _tg, name, payload in read_gpt_partitions(img):
        if name == "BRAINOS-SECURE":
            return payload
    return None


def mask(value):
    """A masked marker for a secret — never the bytes. Reveals only the
    length and, for a long-enough token, a 4-char prefix so you can tell two
    keys apart without exposing either."""
    n = len(value)
    if n >= 12:
        head = value[:4].decode("ascii", "replace")
        return f"<{n} bytes, starts {head!r}, rest hidden>"
    return f"<{n} bytes, hidden>"


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "brainos-key.img")
    if not os.path.exists(path):
        sys.exit(f"no such image: {path}")
    with open(path, "rb") as f:
        img = f.read()
    secure = find_secure(img)
    if secure is None:
        sys.exit("no BRAINOS-SECURE partition in this image")
    records = read_secure_records(secure)
    if not records:
        sys.exit("BRAINOS-SECURE is present but carries no records")

    print(f"{path}: BRAINOS-SECURE — {len(records)} record(s)")
    for rtype, value in records:
        name = SEC_TYPE_NAMES.get(rtype, f"type{rtype}")
        if rtype in SEC_SECRET_TYPES:
            print(f"  [{name}] {mask(value)}")
        elif rtype == SEC_T_BRAINKEY:
            try:
                rec = json.loads(value)
                pub = rec.get("public_key", "")
                inst = rec.get("instance", "?")
                minted = rec.get("minted", "?")
                # the public key is public; the private seed is NOT — mask it
                seed = rec.get("private_seed", "")
                seed_note = f", seed <{len(seed)//2} bytes, hidden>" if seed else ""
                print(f"  [{name}] instance={inst} pub={pub[:16]}...{pub[-8:]} "
                      f"minted={minted}{seed_note} ({len(value)} bytes)")
            except Exception:
                print(f"  [{name}] {len(value)} bytes (unparsed JSON)")
        else:
            preview = value[:32].decode("ascii", "replace")
            print(f"  [{name}] {len(value)} bytes: {preview!r}")


if __name__ == "__main__":
    main()
