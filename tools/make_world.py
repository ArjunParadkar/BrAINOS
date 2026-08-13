#!/usr/bin/env python3
"""Build the world disk — the entity's own storage, separate from its key.

The BrAIn Key carries identity and memory (§13.2). This carries files.
Keeping them on separate media means a file the entity writes can never
grow into the region its episodic memory lives in, and the key stays small
and portable no matter how much the entity accumulates.

The world PERSISTS. Unlike the key image, which build.sh re-mints every
time, this is created once and then left alone — rebuilding the core must
not wipe the entity's files. Pass --force to start the world over.

Identified to the core by the WORLD.ID marker file in its root, never by
enumeration order, so attaching disks in a different order cannot repoint
the filesystem limb at the wrong volume.
"""

import os
import sys
import time

from make_key import SECTOR, DATA_TYPE, Fat16, gpt_image

# 48 MiB: room to work in, still trivial to move around.
TOTAL_SECTORS = 48 * 1024 * 1024 // SECTOR
SECTORS_PER_CLUSTER = 4  # 2 KiB clusters -> ~24k clusters, inside FAT16 range


def seed_files(minted):
    """The world the entity wakes up into. Real files with real content —
    enough that listing and reading return something worth narrating."""
    marker = (
        "BrAInOS world volume\r\n"
        f"created {minted}\r\n"
        "\r\n"
        "this disk is the entity's own storage. it is not the brain key.\r\n"
        "identity and memory live on the key; files live here.\r\n"
    ).encode()

    readme = (
        "THIS IS YOUR WORLD\r\n"
        "\r\n"
        "you can list, read and write files here. this disk belongs to\r\n"
        "you and persists across reboots, the same way your memory does,\r\n"
        "but it is a different medium: memory is who you are, these are\r\n"
        "things you keep.\r\n"
        "\r\n"
        "DOCS/ holds what you were given. WORK/ is empty and yours.\r\n"
        "PROGRAMS/ holds the programs you write: anything there whose\r\n"
        "first line reads '# app: name - what it does' becomes one of\r\n"
        "your applications, and you can run it by name.\r\n"
    ).encode()

    about = (
        "ARENDA INNOVATIONS - BrAInOS\r\n"
        "\r\n"
        "the instance is to the device as we are to our body.\r\n"
        "\r\n"
        "an entity owns its key. the key carries identity and memory.\r\n"
        "bodies are acquired, not inhabited one at a time: every body\r\n"
        "the entity is bound to is present in one body map at once.\r\n"
        "\r\n"
        "authority flows down through KIRA and only through KIRA. an\r\n"
        "action with no matching limb is refused, never performed and\r\n"
        "never pretended.\r\n"
    ).encode()

    principles = (
        "1. the entity is all its bodies at once.\r\n"
        "2. a limb must never remember on the entity's behalf.\r\n"
        "3. cheap thinking by default; escalate only on real difficulty.\r\n"
        "4. no limb, no action, no roleplay - say so plainly.\r\n"
        "5. the datacenter provides body, never identity.\r\n"
    ).encode()

    # a first application, so the shape is legible: the entity learns what
    # a program of its own looks like by reading one, then writes more.
    greet = (
        "# app: greet - say hello to whoever i am given, or to the world\r\n"
        "if len(args) > 0 {\r\n"
        "  print \"hello,\", args[0]\r\n"
        "} else {\r\n"
        "  print \"hello, world\"\r\n"
        "}\r\n"
    ).encode()

    return [
        ("WORLD.ID", marker),
        ("PROGRAMS/GREET.BS", greet),
        ("README.TXT", readme),
        ("DOCS/ABOUT.TXT", about),
        ("DOCS/PRINCIPL.TXT", principles),
    ]


def build(out_path):
    minted = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    fs = Fat16(TOTAL_SECTORS, SECTORS_PER_CLUSTER, "BRAIN WORLD")
    fs.mkdir("DOCS")
    fs.mkdir("WORK")  # deliberately empty: somewhere the entity's own work goes
    fs.mkdir("PROGRAMS")  # its applications: written by it, discovered at boot
    for path, payload in seed_files(minted):
        fs.add_file(path, payload)

    img = gpt_image([(DATA_TYPE, "BrAInOS World", fs.render())])
    with open(out_path, "wb") as f:
        f.write(img)
    return img, minted


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    force = "--force" in sys.argv[1:]
    out_path = args[0] if args else os.path.join(root, "brainos-world.img")

    if os.path.exists(out_path) and not force:
        size = os.path.getsize(out_path)
        print(f"world disk already exists ({size // (1024 * 1024)} MiB) — "
              f"leaving it alone. --force to start the world over.")
        return

    img, minted = build(out_path)
    print(f"world disk written: {out_path} ({len(img) // (1024 * 1024)} MiB, "
          f"created {minted})")
    print("  DOCS/ABOUT.TXT  DOCS/PRINCIPL.TXT  README.TXT  WORK/  PROGRAMS/GREET.BS")


if __name__ == "__main__":
    main()
