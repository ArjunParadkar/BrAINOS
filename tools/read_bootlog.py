#!/usr/bin/env python3
"""
read_bootlog.py -- pull the flight recorder (BRAIN/BOOT.LOG) off a BrAIn
Key after a physical boot, so the bare-metal run can be graded from a real
log instead of memory of what flashed by.

Usage:
  tools/read_bootlog.py                      # the local brainos-key.img
  tools/read_bootlog.py /dev/sdX             # a real stick (needs read perm;
                                             #   sudo, or a sg/disk group)
  tools/read_bootlog.py some-image.img

Reading a raw device only READS -- nothing is written. For a stick, the
whole ESP is at LBA 2048, same as the image (make_key.py's fixed layout).
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from make_key import read_fat16_file  # noqa: E402

ESP_LBA = 2048


def main():
    target = sys.argv[1] if len(sys.argv) > 1 else \
        os.path.join(os.path.dirname(HERE), "brainos-key.img")
    if not os.path.exists(target):
        print(f"no such file or device: {target}", file=sys.stderr)
        return 1
    # a real stick can be tens of GB; the key layout lives in the first
    # ~90 MiB, so snapshot just that much rather than slurping the device
    try:
        big = os.path.getsize(target) > 96 * 1024 * 1024
    except OSError:
        big = True  # block devices report oddly; take the safe path
    if big or not os.path.isfile(target):
        import tempfile
        snap = tempfile.NamedTemporaryFile(suffix=".img", delete=False)
        with open(target, "rb") as src:
            snap.write(src.read(96 * 1024 * 1024))
        snap.close()
        target = snap.name
    data = read_fat16_file(target, ESP_LBA, "BRAIN", "BOOT.LOG")
    if data is None:
        print("no BRAIN/BOOT.LOG on this key -- either the core never got "
              "far enough to flush (firmware-level hang: the log can only "
              "prove how far the core got), or this is not a BrAIn Key.",
              file=sys.stderr)
        return 2
    sys.stdout.write(data.decode("ascii", "replace"))
    print(f"\n--- {len(data)} bytes from {target} ---", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
