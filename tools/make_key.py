#!/usr/bin/env python3
"""
make_key.py — mint a BrAIn Key (BrAInOS Architecture §9).

Builds a universal-boot USB image, pure Python, no root, no mtools:

  GPT
   ├─ p1  BRAINOS-BOOT    64 MiB FAT16 ESP   /EFI/BOOT/BOOTX64.EFI + BOOTAA64.EFI
   │                                          /BRAIN/KEY.PUB  /BRAIN/GENESIS.TXT
   ├─ p2  BRAINOS-CORE    16 MiB FAT16       portable core payloads, one per arch
   └─ p3  BRAINOS-SECURE   4 MiB raw         brain_key (ed25519, software-emulated
                                             secure element — phase 0)

The firmware is the sweep: it picks BOOTX64.EFI or BOOTAA64.EFI by its own
architecture (§9.2). One key, one owner, one entity.
"""

import hashlib
import json
import os
import struct
import sys
import time
import zlib

SECTOR = 512

# ---------------------------------------------------------------- ed25519 ---
# Compact reference implementation (djb) — one keygen at mint time.

q = 2**255 - 19


def inv(x):
    return pow(x, q - 2, q)


d = -121665 * inv(121666) % q
I = pow(2, (q - 1) // 4, q)


def xrecover(y):
    xx = (y * y - 1) * inv(d * y * y + 1)
    x = pow(xx, (q + 3) // 8, q)
    if (x * x - xx) % q != 0:
        x = x * I % q
    if x % 2 != 0:
        x = q - x
    return x


By = 4 * inv(5) % q
B = (xrecover(By), By)


def edwards(P, Q):
    x1, y1 = P
    x2, y2 = Q
    x3 = (x1 * y2 + x2 * y1) * inv(1 + d * x1 * x2 * y1 * y2)
    y3 = (y1 * y2 + x1 * x2) * inv(1 - d * x1 * x2 * y1 * y2)
    return (x3 % q, y3 % q)


def scalarmult(P, e):
    Q = (0, 1)
    while e:
        if e & 1:
            Q = edwards(Q, P)
        P = edwards(P, P)
        e >>= 1
    return Q


def ed25519_keypair():
    seed = os.urandom(32)
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    Ax, Ay = scalarmult(B, a)
    pub = (Ay | ((Ax & 1) << 255)).to_bytes(32, "little")
    return seed, pub


# ------------------------------------------------------------------ FAT16 ---


class Fat16:
    """Minimal FAT16 volume builder. 8.3 names only, one cluster per subdir."""

    def __init__(self, total_sectors, sectors_per_cluster, label, hidden=0):
        self.total = total_sectors
        self.spc = sectors_per_cluster
        self.label = label.ljust(11)[:11].encode()
        self.hidden = hidden
        self.root_entries = 512
        self.root_sectors = self.root_entries * 32 // SECTOR
        # FAT size: iterate once to a fixed point
        fat_sectors = 1
        for _ in range(8):
            data = self.total - 1 - 2 * fat_sectors - self.root_sectors
            clusters = data // self.spc
            fat_sectors = (clusters + 2) * 2 + SECTOR - 1
            fat_sectors //= SECTOR
        self.fat_sectors = fat_sectors
        self.clusters = (self.total - 1 - 2 * fat_sectors - self.root_sectors) // self.spc
        if not 4085 <= self.clusters <= 65524:
            raise ValueError(f"cluster count {self.clusters} outside FAT16 range")
        self.fat = [0] * (self.clusters + 2)
        self.fat[0] = 0xFFF8
        self.fat[1] = 0xFFFF
        self.data = bytearray(self.clusters * self.spc * SECTOR)
        self.root = bytearray(self.root_entries * 32)
        self.root_used = 0
        self.next_cluster = 2
        # path -> (cluster, bytearray view offset for its entry table)
        self.dirs = {"": None}  # root

    def _alloc_chain(self, nbytes):
        n = max(1, -(-nbytes // (self.spc * SECTOR)))
        first = self.next_cluster
        if first + n - 2 > self.clusters + 1:
            raise ValueError("volume full")
        for i in range(n):
            c = self.next_cluster + i
            self.fat[c] = c + 1
        self.fat[self.next_cluster + n - 1] = 0xFFFF
        self.next_cluster += n
        return first, n

    def _write_data(self, first_cluster, payload):
        off = (first_cluster - 2) * self.spc * SECTOR
        self.data[off : off + len(payload)] = payload

    @staticmethod
    def _entry(name83, attr, cluster, size):
        e = bytearray(32)
        e[0:11] = name83
        e[11] = attr
        # timestamp: fixed mint date is fine
        date = ((2026 - 1980) << 9) | (7 << 5) | 6
        struct.pack_into("<H", e, 24, date)  # write date
        struct.pack_into("<H", e, 26, cluster)
        struct.pack_into("<I", e, 28, size)
        return bytes(e)

    @staticmethod
    def name83(name):
        name = name.upper()
        if "." in name:
            base, ext = name.rsplit(".", 1)
        else:
            base, ext = name, ""
        if len(base) > 8 or len(ext) > 3:
            raise ValueError(f"{name} not 8.3")
        return base.ljust(8).encode() + ext.ljust(3).encode()

    def _add_entry(self, dirpath, entry):
        if dirpath == "":
            if self.root_used >= self.root_entries:
                raise ValueError("root dir full")
            o = self.root_used * 32
            self.root[o : o + 32] = entry
            self.root_used += 1
        else:
            cluster, used = self.dirs[dirpath]
            cap = self.spc * SECTOR // 32
            if used >= cap:
                raise ValueError("subdir full")
            off = (cluster - 2) * self.spc * SECTOR + used * 32
            self.data[off : off + 32] = entry
            self.dirs[dirpath] = (cluster, used + 1)

    def mkdir(self, path):
        parent, _, name = path.rpartition("/")
        cluster, _ = self._alloc_chain(1)
        parent_cluster = 0 if parent == "" else self.dirs[parent][0]
        table = bytearray()
        table += self._entry(b".          ", 0x10, cluster, 0)
        table += self._entry(b"..         ", 0x10, parent_cluster, 0)
        self._write_data(cluster, table)
        self.dirs[path] = (cluster, 2)
        self._add_entry(parent, self._entry(self.name83(name), 0x10, cluster, 0))

    def add_file(self, path, payload):
        parent, _, name = path.rpartition("/")
        if len(payload) == 0:
            self._add_entry(parent, self._entry(self.name83(name), 0x20, 0, 0))
            return
        cluster, _ = self._alloc_chain(len(payload))
        self._write_data(cluster, payload)
        self._add_entry(
            parent, self._entry(self.name83(name), 0x20, cluster, len(payload))
        )

    def render(self):
        bs = bytearray(SECTOR)
        bs[0:3] = b"\xeb\x3c\x90"
        bs[3:11] = b"BRAINOS "
        struct.pack_into("<H", bs, 11, SECTOR)
        bs[13] = self.spc
        struct.pack_into("<H", bs, 14, 1)  # reserved sectors
        bs[16] = 2  # FATs
        struct.pack_into("<H", bs, 17, self.root_entries)
        struct.pack_into("<H", bs, 19, self.total if self.total < 65536 else 0)
        bs[21] = 0xF8
        struct.pack_into("<H", bs, 22, self.fat_sectors)
        struct.pack_into("<H", bs, 24, 63)
        struct.pack_into("<H", bs, 26, 255)
        struct.pack_into("<I", bs, 28, self.hidden)
        struct.pack_into("<I", bs, 32, self.total if self.total >= 65536 else 0)
        bs[36] = 0x80
        bs[38] = 0x29
        struct.pack_into("<I", bs, 39, 0xB4A1B005)
        bs[43:54] = self.label
        bs[54:62] = b"FAT16   "
        bs[510] = 0x55
        bs[511] = 0xAA

        fat = bytearray(self.fat_sectors * SECTOR)
        for i, v in enumerate(self.fat):
            struct.pack_into("<H", fat, i * 2, v)

        out = bytes(bs) + bytes(fat) * 2 + bytes(self.root) + bytes(self.data)
        out += b"\0" * (self.total * SECTOR - len(out))  # cluster-rounding slack
        return out


# -------------------------------------------------------------------- GPT ---


def pack_guid(s):
    """RFC4122 string -> GPT on-disk mixed-endian bytes."""
    p = s.split("-")
    return (
        struct.pack("<IHH", int(p[0], 16), int(p[1], 16), int(p[2], 16))
        + bytes.fromhex(p[3])
        + bytes.fromhex(p[4])
    )


ESP_TYPE = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
DATA_TYPE = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"
SECURE_TYPE = "6B7A19F5-3C2E-4D10-9A4B-53E6F2D80B1C"  # BrAInOS secure area


def rand_guid():
    b = bytearray(os.urandom(16))
    b[6] = (b[6] & 0x0F) | 0x40
    b[8] = (b[8] & 0x3F) | 0x80
    h = b.hex()
    return f"{h[0:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}".upper()


def gpt_image(partitions):
    """partitions: list of (type_guid, name, payload_bytes). Returns full image."""
    start_lba = 2048
    layout = []
    lba = start_lba
    for tg, name, payload in partitions:
        n = -(-len(payload) // SECTOR)
        layout.append((tg, name, lba, lba + n - 1, payload))
        lba = lba + n
        lba = (lba + 2047) // 2048 * 2048  # 1 MiB align each partition
    last_data_lba = layout[-1][3]
    total = last_data_lba + 1 + 33 + 63
    total = -(-total // 2048) * 2048  # round image to 1 MiB

    entries = bytearray(128 * 128)
    for i, (tg, name, first, last, _) in enumerate(layout):
        o = i * 128
        entries[o : o + 16] = pack_guid(tg)
        entries[o + 16 : o + 32] = pack_guid(rand_guid())
        struct.pack_into("<QQ", entries, o + 32, first, last)
        struct.pack_into("<Q", entries, o + 48, 0)
        n = name.encode("utf-16le")[:70]
        entries[o + 56 : o + 56 + len(n)] = n

    def header(current, backup, entries_lba):
        h = bytearray(92)
        h[0:8] = b"EFI PART"
        struct.pack_into("<I", h, 8, 0x00010000)
        struct.pack_into("<I", h, 12, 92)
        struct.pack_into("<QQ", h, 24, current, backup)
        struct.pack_into("<QQ", h, 40, 34, total - 34)
        h[56:72] = pack_guid(rand_guid())
        struct.pack_into("<Q", h, 72, entries_lba)
        struct.pack_into("<II", h, 80, 128, 128)
        struct.pack_into("<I", h, 84 + 4, zlib.crc32(entries))
        struct.pack_into("<I", h, 16, zlib.crc32(h))
        return bytes(h) + b"\0" * (SECTOR - 92)

    # protective MBR
    mbr = bytearray(SECTOR)
    mbr[446] = 0x00
    mbr[446 + 1 : 446 + 4] = b"\x00\x02\x00"
    mbr[446 + 4] = 0xEE
    mbr[446 + 5 : 446 + 8] = b"\xff\xff\xff"
    struct.pack_into("<II", mbr, 446 + 8, 1, min(total - 1, 0xFFFFFFFF))
    mbr[510] = 0x55
    mbr[511] = 0xAA

    img = bytearray(total * SECTOR)
    img[0:SECTOR] = mbr
    img[SECTOR : 2 * SECTOR] = header(1, total - 1, 2)
    img[2 * SECTOR : 2 * SECTOR + len(entries)] = entries
    for _, _, first, _, payload in layout:
        img[first * SECTOR : first * SECTOR + len(payload)] = payload
    bk_entries_lba = total - 33
    img[bk_entries_lba * SECTOR : bk_entries_lba * SECTOR + len(entries)] = entries
    img[(total - 1) * SECTOR : total * SECTOR] = header(total - 1, 1, bk_entries_lba)
    return bytes(img)


# -------------------------------------------------- memory carry-over -----


JOURNAL_MAGIC = b"BRNJRNL1"
JOURNAL_MAX_PAYLOAD = 64 * 1024


def journal_open(raw):
    """Python twin of mind/src/journal.rs::open — validate one slot.
    Returns (generation, payload) or None. The CRC covers gen|len|payload."""
    import zlib
    if not raw or len(raw) < 24 or raw[:8] != JOURNAL_MAGIC:
        return None
    gen, ln, crc = struct.unpack_from("<QII", raw, 8)
    if ln > JOURNAL_MAX_PAYLOAD or len(raw) < 24 + ln:
        return None
    payload = raw[24 : 24 + ln]
    if (zlib.crc32(raw[8:20] + payload) & 0xFFFFFFFF) != crc:
        return None
    return gen, payload


def newest_memory(img_path, part_lba, stem, legacy_name):
    """The newest valid journal record for a memory file, falling back to
    the pre-journal flat file. Mirrors journal.rs::newest + legacy load."""
    recs = []
    for slot in ("A", "B"):
        raw = read_fat16_file(img_path, part_lba, "BRAIN", f"{stem}_{slot}.JNL")
        rec = journal_open(raw) if raw else None
        if rec:
            recs.append(rec)
    if recs:
        return max(recs, key=lambda r: r[0])[1]
    return read_fat16_file(img_path, part_lba, "BRAIN", legacy_name)


def read_fat16_file(img_path, part_lba, dirname, filename):
    """Minimal FAT16 reader: pull one file out of an existing key image so
    the entity's memories survive a rebuild. Returns bytes or None."""
    try:
        with open(img_path, "rb") as f:
            img = f.read()
    except FileNotFoundError:
        return None
    base = part_lba * SECTOR
    if img[base + 54 : base + 59] != b"FAT16":
        return None
    spc = img[base + 13]
    reserved = struct.unpack_from("<H", img, base + 14)[0]
    nfats = img[base + 16]
    root_entries = struct.unpack_from("<H", img, base + 17)[0]
    fat_size = struct.unpack_from("<H", img, base + 22)[0]
    fat_off = base + reserved * SECTOR
    root_off = fat_off + nfats * fat_size * SECTOR
    data_off = root_off + root_entries * 32

    def cluster_bytes(first):
        out = b""
        c = first
        seen = 0
        while 2 <= c < 0xFFF8 and seen < 65536:
            out += img[data_off + (c - 2) * spc * SECTOR :][: spc * SECTOR]
            c = struct.unpack_from("<H", img, fat_off + c * 2)[0]
            seen += 1
        return out

    def find(entries, name83, want_dir):
        for i in range(0, len(entries), 32):
            e = entries[i : i + 32]
            if len(e) < 32 or e[0] in (0, 0xE5):
                continue
            if e[0:11] == name83 and bool(e[11] & 0x10) == want_dir:
                cluster = struct.unpack_from("<H", e, 26)[0]
                size = struct.unpack_from("<I", e, 28)[0]
                return cluster, size
        return None

    root = img[root_off : root_off + root_entries * 32]
    if dirname in ("", "\\", "/"):
        # a file in the volume root — the root directory is the fixed region,
        # not a cluster chain, so search it directly rather than descending.
        entries = root
    else:
        # descend one component at a time: "WORK/SUB" (or backslashes)
        entries = root
        for comp in dirname.replace("\\", "/").strip("/").split("/"):
            d = find(entries, Fat16.name83(comp), True)
            if not d:
                return None
            entries = cluster_bytes(d[0])
    hit = find(entries, Fat16.name83(filename), False)
    if not hit:
        return None
    return cluster_bytes(hit[0])[: hit[1]]


# ------------------------------------------------------------------- main ---


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    keep = "--keep-key" in sys.argv[1:]
    out_path = args[0] if args else os.path.join(root, "brainos-key.img")
    x64 = os.path.join(root, "core/target/x86_64-unknown-uefi/release/brainos.efi")
    a64 = os.path.join(root, "core/target/aarch64-unknown-uefi/release/brainos.efi")
    for p in (x64, a64):
        if not os.path.exists(p):
            sys.exit(f"missing {p} — build the core first (see build.sh)")
    core_x64 = open(x64, "rb").read()
    core_a64 = open(a64, "rb").read()

    # --- identity: same entity across rebuilds unless told otherwise ---
    kpath0 = os.path.join(root, "key", "brain_key.json")
    episodes = None
    notes = None
    if keep and os.path.exists(kpath0):
        rec = json.load(open(kpath0))
        seed = bytes.fromhex(rec["private_seed"])
        pub = bytes.fromhex(rec["public_key"])
        minted = rec.get("minted", "unknown")
        # newest journal slot wins; the flat file is the pre-journal legacy.
        # Both memories (episodes) and the notebook travel across rebuilds —
        # rebuilding the OS must never change who the entity is (§13.2).
        episodes = newest_memory(out_path, 2048, "EPI", "EPISODES.LOG")
        notes = newest_memory(out_path, 2048, "NOTE", "NOTES.TXT")
        print(f"keeping existing BrAIn Key {rec['public_key'][:16]}... "
              f"({len(episodes or b'')} bytes of memories, "
              f"{len(notes or b'')} bytes of notebook carried over)")
    else:
        print("minting BrAIn Key (ed25519) ...")
        seed, pub = ed25519_keypair()
        minted = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    # the instance's name is part of its identity and travels with the key
    name_arg = next((a.split("=", 1)[1] for a in sys.argv[1:]
                     if a.startswith("--name=")), None)
    if keep and os.path.exists(kpath0):
        instance_name = name_arg or json.load(open(kpath0)).get("instance", "Blur")
    else:
        instance_name = name_arg or "Blur"
    key_record = {
        "brainos": "brain_key",
        "version": 1,
        "alg": "ed25519",
        "instance": instance_name,
        "public_key": pub.hex(),
        "private_seed": seed.hex(),
        "minted": minted,
        "secure_element": "software-emulated (phase 0) — move to TPM/enclave later",
        "rule": "one key, one owner, one entity",
    }
    keydir = os.path.join(root, "key")
    os.makedirs(keydir, exist_ok=True)
    kpath = os.path.join(keydir, "brain_key.json")
    with open(kpath, "w") as f:
        json.dump(key_record, f, indent=2)
    os.chmod(kpath, 0o600)

    genesis = (
        "BrAInOS state graph - genesis node\r\n"
        f"entity minted {minted}\r\n"
        "beliefs: none yet. memories: none yet. bodies: whichever machine\r\n"
        "this key is plugged into. there is no filesystem; you are reading\r\n"
        "the seed of a state graph.\r\n"
    ).encode()

    readme = (
        "BRAINOS BRAIN KEY - universal boot media\r\n"
        "\r\n"
        "plug into any UEFI machine (x86_64 or aarch64), boot from USB,\r\n"
        "secure boot off. the firmware picks the right core by itself.\r\n"
        "the entity wakes, warms the room, and says BRAINOS.\r\n"
    ).encode()

    # --- partition 1: ESP ---
    esp = Fat16(64 * 1024 * 2, 4, "BRAINOS EFI", hidden=2048)
    esp.mkdir("EFI")
    esp.mkdir("EFI/BOOT")
    esp.add_file("EFI/BOOT/BOOTX64.EFI", core_x64)
    esp.add_file("EFI/BOOT/BOOTAA64.EFI", core_a64)
    esp.mkdir("BRAIN")
    esp.add_file("BRAIN/KEY.PUB", (pub.hex() + "\r\n").encode())
    # phase 0: the seed rides the ESP so the core can sign (software-
    # emulated secure element). A real secure element replaces this file.
    esp.add_file("BRAIN/SEED.HEX", (seed.hex() + "\r\n").encode())
    esp.add_file("BRAIN/NAME.TXT", (instance_name + "\r\n").encode())
    esp.add_file("BRAIN/GENESIS.TXT", genesis)
    # migrated memory rides in as the legacy flat files: a fresh image has
    # no journal slots yet, so the core reads these, then journals onward.
    if episodes:
        esp.add_file("BRAIN/EPISODES.LOG", episodes)
    if notes:
        esp.add_file("BRAIN/NOTES.TXT", notes)
    esp.add_file("README.TXT", readme)

    # --- partition 2: core payloads, one per supported arch (§9.1) ---
    corefs = Fat16(16 * 1024 * 2, 1, "BRAINOSCORE")
    corefs.add_file("CORE_X64.EFI", core_x64)
    corefs.add_file("CORE_A64.EFI", core_a64)
    corefs.add_file(
        "MANIFEST.TXT",
        b"portable_core.x86_64 -> CORE_X64.EFI\r\n"
        b"portable_core.aarch64 -> CORE_A64.EFI\r\n"
        b"one source, compiled per target. cognition never touches an ISA.\r\n",
    )

    # --- partition 3: secure area (software-emulated, phase 0) ---
    secure = json.dumps(key_record, indent=2).encode()
    secure += b"\n" + b"\0" * (4 * 1024 * 1024 - len(secure) - 1)

    img = gpt_image(
        [
            (ESP_TYPE, "BRAINOS-BOOT", esp.render()),
            (DATA_TYPE, "BRAINOS-CORE", corefs.render()),
            (SECURE_TYPE, "BRAINOS-SECURE", secure),
        ]
    )
    with open(out_path, "wb") as f:
        f.write(img)
    print(f"key minted: {out_path} ({len(img)//(1024*1024)} MiB)")
    print(f"entity id (ed25519 pub): {pub.hex()[:16]}...{pub.hex()[-8:]}")
    print(f"private seed kept at:    {kpath}  (chmod 600 — guard it)")
    print("\nwrite it to a USB stick (DOUBLE-CHECK the device!):")
    print(f"  sudo dd if={out_path} of=/dev/sdX bs=4M oflag=sync status=progress")


if __name__ == "__main__":
    main()
