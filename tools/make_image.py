#!/usr/bin/env python3
"""
make_image.py — assemble the full Stage 3 USB image, brainos.img.

Takes a minted BrAIn Key image (from make_key.py, built with --body-efi so its
p1 ESP already carries /EFI/BRAIN/BODY.EFI) and a body rootfs ext4 image
(produced elsewhere — e.g. mkosi / mkfs.ext4 -d), and emits a GPT image:

  GPT (brainos.img)
   ├─ p1  BRAINOS-BOOT    ESP FAT16   (verbatim from the key: core + BODY.EFI + /BRAIN)
   ├─ p2  BRAINOS-BODY    ext4        (the body rootfs; type = Linux root x86-64
   │                                   4f68bce3-…-fbcaf984b709, PARTLABEL BRAINOS-BODY)
   └─ p3  BRAINOS-SECURE  4 MiB raw   (verbatim from the key: brain key + API key)

LAYOUT CHOICE — the old p2 BRAINOS-CORE (spare copies of the core payload, one
per arch) is DROPPED in this layout. In a key it is a redundancy/recovery copy
of the same bytes already at p1 /EFI/BOOT; in the production image the second
partition is where the body's rootfs must live, and the core already ships on
p1 where the firmware actually loads it. Nothing reads BRAINOS-CORE at run time,
so removing it costs no capability and frees the slot for the body. (If a
recovery copy is ever wanted again it belongs AFTER the body, as p4.)

The p1 ESP and p3 secure area are reused byte-for-byte from the key, so this
tool never re-mints identity, never re-reads the API key, and never rebuilds
the FAT ESP — it only re-frames the existing partitions plus the body into a
fresh GPT. make_key.py already wrote the layout description into p1's README.TXT
when it was built with --body-efi.

Usage:
  tools/make_image.py --key brainos-key.img --body body.ext4 -o brainos.img
"""
import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from make_key import (  # noqa: E402
    gpt_image,
    read_gpt_partitions,
    ESP_TYPE,
    BODY_TYPE,
    SECURE_TYPE,
)


def _part_by_name(parts, name):
    for tg, pname, payload in parts:
        if pname == name:
            return tg, pname, payload
    return None


def main():
    ap = argparse.ArgumentParser(description="assemble brainos.img (key + body)")
    ap.add_argument("--key", default=os.path.join(ROOT, "brainos-key.img"),
                    help="minted key image (make_key.py, ideally --body-efi)")
    ap.add_argument("--body", required=True,
                    help="body rootfs ext4 image (raw bytes become p2)")
    ap.add_argument("-o", "--out", default=os.path.join(ROOT, "brainos.img"),
                    help="output image path (default brainos.img)")
    args = ap.parse_args()

    if not os.path.exists(args.key):
        sys.exit(f"key image not found: {args.key} (run ./build.sh first)")
    if not os.path.exists(args.body):
        sys.exit(f"body image not found: {args.body}")

    with open(args.key, "rb") as f:
        key_img = f.read()
    parts = read_gpt_partitions(key_img)

    boot = _part_by_name(parts, "BRAINOS-BOOT")
    secure = _part_by_name(parts, "BRAINOS-SECURE")
    if boot is None:
        sys.exit("key image has no BRAINOS-BOOT (p1 ESP) partition")
    if secure is None:
        sys.exit("key image has no BRAINOS-SECURE (p3) partition")

    with open(args.body, "rb") as f:
        body = f.read()
    if len(body) < 512 or body[0x438:0x43A] != b"\x53\xef":
        # ext superblock magic 0xEF53 lives at byte offset 0x438 (LE)
        print("  warning: body image lacks an ext2/3/4 superblock magic — "
              "writing it anyway, but double-check it is a real rootfs",
              file=sys.stderr)

    esp_payload = boot[2]
    secure_payload = secure[2]

    img = gpt_image([
        (ESP_TYPE, "BRAINOS-BOOT", esp_payload),
        (BODY_TYPE, "BRAINOS-BODY", body),
        (SECURE_TYPE, "BRAINOS-SECURE", secure_payload),
    ])
    with open(args.out, "wb") as f:
        f.write(img)

    def mib(n):
        return n / (1024 * 1024)

    print(f"image assembled: {args.out} ({mib(len(img)):.0f} MiB)")
    print(f"  p1 BRAINOS-BOOT    {mib(len(esp_payload)):7.0f} MiB  ESP (core + BODY.EFI + key)")
    print(f"  p2 BRAINOS-BODY    {mib(len(body)):7.0f} MiB  ext4 rootfs (PARTLABEL BRAINOS-BODY)")
    print(f"  p3 BRAINOS-SECURE  {mib(len(secure_payload)):7.0f} MiB  brain key + optional API key")
    print("  (old p2 BRAINOS-CORE dropped — see make_image.py docstring)")
    print("\nwrite it to a USB stick (DOUBLE-CHECK the device!):")
    print(f"  sudo dd if={args.out} of=/dev/sdX bs=4M oflag=sync status=progress")


if __name__ == "__main__":
    main()
