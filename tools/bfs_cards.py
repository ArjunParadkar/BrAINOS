#!/usr/bin/env python3
"""BFS cards: one standard header on every source file, and the code map.

BFS = "BrAInOS From Scratch". The goal is that any file can be opened cold
and answer five questions in its first six lines:

    //! BFS  LEVEL    which domain / stage of the architecture this belongs to
    //! BFS  PURPOSE  what this file is for, in one line
    //! BFS  DEFINES  the public items it declares
    //! BFS  USES     what it imports (this crate | other BrAInOS crates | external)
    //! BFS  USED BY  which files import it

LEVEL comes from the table below (one entry per crate). PURPOSE is written by
a human and is preserved; the first time a file gets a card it is seeded from
the file's own first doc line. DEFINES, USES and USED BY are computed from the
source, so they are never typed by hand and cannot drift:

    tools/bfs_cards.py            rewrite every card and docs/CODE_MAP.md
    tools/bfs_cards.py --check    exit 1 if anything is out of date (CI/test)

Rust files carry the card in the file. Python and C files are listed in
docs/CODE_MAP.md only and are NOT modified: several of them are frozen by
content hash for cluster campaigns (blur/hpcc CODE_SHA), and a comment edit
would change that hash.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# (path prefix, level). First match wins, so longer prefixes go first.
LEVELS = [
    ("os/abi", "Foundation - shared vocabulary (ids, time, verbs, codec, errors)"),
    ("os/transport", "Foundation - the wire (frames, sessions, links)"),
    ("os/crypto", "Domain 1 Silicon - the BrAIn Key, hashes, Merkle log, quorum"),
    ("os/rt", "Domain 2 Autonomic - envelopes, reflexes, scheduler, watchdogs"),
    ("os/bodyd", "Domain 2 Autonomic - the runtime on the body's own computer"),
    ("os/blur", "Domain 2 Autonomic - the BLUR quadruped body (wire, acquisition, supervisor)"),
    ("os/part", "Parts and nerves - every piece of hardware as a body part"),
    ("os/bpu", "Domain 4 substrate - the BPU seam (descriptor, mock, nerve, planner)"),
    ("os/motor", "Domain 2 Autonomic - motor memory (self-model, mandates, primitives, reflexes)"),
    ("os/kira", "Domain 3 KIRA - the eight-stage gate; the only path to action"),
    ("os/cognition", "Domain 4 Cognition - tiers, placement, attention, escalation"),
    ("os/memory", "Domain 5 State - the state graph; the only memory"),
    ("os/body", "Section 8 Acquisition - a device becomes a region of the body"),
    ("os/adapters", "Section 8 Acquisition - adapters (ROS 2, MHS, the model boundary)"),
    ("os/worlds", "Section 3.8 - the three worlds (self, human, internet)"),
    ("os/entity", "Stage 4 Entity - one Key, one state graph, every body at once"),
    ("os/proof", "Stage 6 - property domains (bounded checking, not a proof)"),
    ("os/fleet", "Stage 7 - fleet registry and skill marketplace"),
    ("os/export", "Freedom principle - entity export"),
    ("os/tools/read-export", "Freedom principle - export reader with no BrAInOS crate"),
    ("os/nostd", "Discipline - the no_std canary and its checks"),
    ("os/host", "Host - the only crate with an operating system (CLI, scenarios, OS nerves)"),
    ("mind", "Seed (Stages 1-2) - the portable no_std mind"),
    ("core", "Seed (Stages 1-2) - Domain 1 flesh: the UEFI boot core"),
    ("miniblur/core", "Phase 0 - Mini-Blur hexapod as numbers (shared by firmware and seat)"),
    ("miniblur/sim", "Phase 0 - the simulated Mini-Blur body"),
    ("miniblur", "Phase 0 - Mini-Blur tooling"),
    ("firmware/miniblur-a", "Phase 0 - Domain 2 firmware for the ESP32-S3"),
    ("firmware", "BLUR firmware - motor nodes and Teensy body controller"),
    ("body/operator", "Operator (Stage 3-4 milestones) - the seat daemon on the Linux body"),
    ("body/face", "Operator - the GTK face"),
    ("body", "Operator - Linux body build and session"),
    ("reach", "Stage 4 - Reach: any Python device as a body"),
    ("blur/pi", "BLUR - Pi-side stack (KIRA mirror, skills, voice, vision, mind)"),
    ("blur/sim", "BLUR - the MuJoCo twin behind the real wire formats"),
    ("blur/learn", "BLUR - skill training (PPO, tasks, evaluation)"),
    ("blur/hpcc", "BLUR - cluster campaigns"),
    ("blur", "BLUR - descriptor, tools, prototypes"),
    ("motor/sim", "Motor - the simulated desk arm (physics only; the seat and the arm's loop are Rust)"),
    ("motor/gait", "Motor - gait training from the self-model (PPO, int8 export, evaluation)"),
    ("motor/hpcc", "Motor - cluster runs"),
    ("motor", "Motor - self-model checks and tooling outside os/"),
    ("tools", "Seed tooling - key/image builders, tether daemon, test suites"),
]

SKIP_DIRS = {
    "target", ".git", ".venv", "venv", "__pycache__", ".pio-core", ".pio", "node_modules",
    "tts-env", "piper", "qemu-local", "workspace", "iso_build", "work", "runs", "logs",
    ".cargo", "campaigns", "audition",
}
GENERATED = re.compile(r"(^|/)(gen|refs_gen|tables_gen)\.rs$")
CARD = re.compile(r"^//! BFS  (LEVEL|PURPOSE|DEFINES|USES|USED BY) ?(.*)$")
PUB_RS = re.compile(
    r"^pub(?:\([a-z:]+\))? (?:async )?(?:unsafe )?(?:const )?(fn|struct|enum|trait|type|const|static|mod|union) (\w+)",
    re.M,
)
USE_RS = re.compile(r"^\s*(?:pub )?use ([A-Za-z_][\w]*)(?:::\{?\s*([A-Za-z_]\w*))?", re.M)
PATH_RS = re.compile(r"\b(brainos_\w+|miniblur_core|crate|super)::(\w+)")


def level_for(rel):
    for prefix, lvl in LEVELS:
        if rel == prefix or rel.startswith(prefix + "/"):
            return lvl
    return "Unclassified"


def walk(exts):
    out = []
    for d, dirs, files in os.walk(ROOT):
        dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS)
        for f in sorted(files):
            if f.endswith(exts):
                out.append(os.path.relpath(os.path.join(d, f), ROOT))
    return out


def strip_card(text):
    """Return (purpose or None, text without the card)."""
    lines = text.split("\n")
    purpose = None
    i = 0
    while i < len(lines) and CARD.match(lines[i]):
        m = CARD.match(lines[i])
        if m.group(1) == "PURPOSE":
            purpose = m.group(2).strip()
        i += 1
    if i and i < len(lines) and lines[i].strip() == "//!":
        i += 1
    return purpose, "\n".join(lines[i:])


def code_only(src):
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    return "\n".join(l.split("//")[0] for l in src.split("\n"))


def seed_purpose(body):
    for line in body.split("\n"):
        s = line.strip()
        if s.startswith("//!"):
            t = s[3:].strip()
            if t and not t.startswith(("#", "```", "[")):
                t = re.sub(r"^`?[\w/-]+`?\s+[—-]+\s+", "", t) if re.match(r"^`?brainos-", t) else t
                return t.rstrip(".").strip()
        elif s and not s.startswith("//"):
            break
    return "(not yet described - write one line here)"


def crate_of(rel):
    """Nearest directory at or above the file that has a Cargo.toml with [package]."""
    d = os.path.dirname(rel)
    while d:
        p = os.path.join(ROOT, d, "Cargo.toml")
        if os.path.exists(p):
            t = open(p).read()
            m = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', t, re.M | re.S)
            if m:
                return d, m.group(1)
        d = os.path.dirname(d)
    return None, None


def module_path(rel, crate_dir):
    sub = os.path.relpath(rel, crate_dir)
    parts = sub[:-3].split("/")
    if parts[0] == "src":
        parts = parts[1:]
    else:
        return None  # tests/, benches/, build.rs: not importable modules
    if parts and parts[-1] == "mod":
        parts = parts[:-1]
    if parts in (["lib"], ["main"]):
        return []
    return parts


def short(rel, crate_dir):
    base = os.path.basename(crate_dir)
    sub = os.path.relpath(rel, crate_dir)
    sub = sub[4:] if sub.startswith("src/") else sub
    return base + "/" + sub[:-3]


def clip(items, n):
    items = list(items)
    if len(items) <= n:
        return ", ".join(items) if items else "-"
    return ", ".join(items[:n]) + f", +{len(items) - n} more"


def build():
    rs = [f for f in walk((".rs",)) if not GENERATED.search(f)]
    info = {}
    for rel in rs:
        text = open(os.path.join(ROOT, rel)).read()
        purpose, body = strip_card(text)
        crate_dir, pkg = crate_of(rel)
        info[rel] = {
            "text": text, "body": body, "code": code_only(body),
            "purpose": purpose or seed_purpose(body),
            "crate_dir": crate_dir, "pkg": pkg,
            "mod": module_path(rel, crate_dir) if crate_dir else None,
        }

    # which crates depend on which package (for lib.rs "used by")
    dependents = {}
    for d, dirs, files in os.walk(ROOT):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        if "Cargo.toml" in files:
            t = open(os.path.join(d, "Cargo.toml")).read()
            me = re.search(r'^name\s*=\s*"([^"]+)"', t, re.M)
            if not me:
                continue
            for dep in re.findall(r"^([\w-]+)\s*=\s*\{[^}]*(?:workspace|path)", t, re.M):
                dependents.setdefault(dep, set()).add(os.path.basename(d))

    for rel, i in info.items():
        defines = [m.group(2) for m in PUB_RS.finditer(i["body"])]
        own, ours, ext = set(), set(), set()
        local_mods = set(re.findall(r"^\s*(?:pub(?:\([a-z]+\))? )?mod (\w+)\s*[;{]", i["code"], re.M))
        for m in USE_RS.finditer(i["code"]):
            head, nxt = m.group(1), m.group(2)
            if head in local_mods:
                own.add(head)
            elif head in ("crate", "super"):
                if nxt:
                    own.add(nxt)
            elif head.startswith("brainos_") or head == "miniblur_core":
                ours.add(head.replace("brainos_", "").replace("_", "-"))
            elif head in ("core", "alloc", "self"):
                pass
            else:
                ext.add(head.replace("_", "-"))
        for m in PATH_RS.finditer(i["code"]):
            head, nxt = m.group(1), m.group(2)
            if head in ("crate", "super"):
                if nxt and nxt[0].islower():
                    own.add(nxt)
            else:
                ours.add(head.replace("brainos_", "").replace("_", "-"))
        uses = []
        if own:
            uses.append("this crate: " + clip(sorted(own), 8))
        if ours:
            uses.append("brainos: " + clip(sorted(ours), 10))
        if ext:
            uses.append("external: " + clip(sorted(ext), 6))

        used = set()
        mod, pkg = i["mod"], i["pkg"]
        if mod == [] and pkg:
            used = {"crate " + c for c in dependents.get(pkg, set())}
        elif mod and pkg:
            ident = pkg.replace("-", "_")
            path = "::".join(mod)
            pats_other = re.compile(r"\b" + re.escape(ident) + r"::" + re.escape(path) + r"\b")
            pats_same = re.compile(r"\bcrate::" + re.escape(path) + r"\b|\bsuper::" + re.escape(mod[-1]) + r"\b")
            reexport = re.compile(r"\bpub use " + re.escape(mod[-1]) + r"::")
            for other, o in info.items():
                if other == rel or not o["crate_dir"]:
                    continue
                if o["pkg"] == pkg:
                    if pats_same.search(o["code"]) or reexport.search(o["code"]):
                        used.add(short(other, o["crate_dir"]))
                    elif o["mod"] is None and pats_other.search(o["code"]):
                        used.add(short(other, o["crate_dir"]))
                elif pats_other.search(o["code"]):
                    used.add(short(other, o["crate_dir"]))
        i["level"] = level_for(rel) + (f"  [crate {pkg}]" if pkg else "")
        i["defines"] = clip(defines, 12)
        i["uses"] = " | ".join(uses) if uses else "nothing outside core/alloc"
        i["used_by"] = clip(sorted(used), 8) if used else (
            "(entry point or test)" if mod is None or rel.endswith("main.rs") else "-")
        card = "\n".join([
            "//! BFS  LEVEL    " + i["level"],
            "//! BFS  PURPOSE  " + i["purpose"],
            "//! BFS  DEFINES  " + i["defines"],
            "//! BFS  USES     " + i["uses"],
            "//! BFS  USED BY  " + i["used_by"],
            "//!",
        ])
        i["new"] = card + "\n" + i["body"]
    return info


def others():
    """Python and C sources: map entries only, files untouched."""
    out = []
    for rel in walk((".py", ".c", ".h", ".cpp", ".ino", ".sh")):
        text = open(os.path.join(ROOT, rel), errors="replace").read()
        purpose = "(not yet described)"
        m = re.search(r'^(?:[rbu]*)("""|\'\'\')\s*(.+)', text, re.M)
        if rel.endswith(".py") and m:
            purpose = m.group(2).strip().strip('"\'').rstrip(".")
        else:
            for line in text.split("\n")[:25]:
                s = line.strip()
                if s.startswith("#!") or not s:
                    continue
                c = re.match(r"^(?:#|//|/\*+|\*)\s*(.{8,})$", s)
                if c:
                    purpose = c.group(1).rstrip("*/ ").rstrip(".")
                    break
                break
        if rel.endswith(".py"):
            defs = re.findall(r"^(?:def|class) (\w+)", text, re.M)
        else:
            defs = re.findall(r"^[A-Za-z_][\w \*]*?\b(\w+)\s*\([^;{]*\)\s*\{", text, re.M)
        out.append((rel, level_for(rel), purpose, clip([d for d in defs if not d.startswith("_")], 8)))
    return out


def render_map(info):
    L = [
        "# BrAInOS code map",
        "",
        "Generated by `tools/bfs_cards.py`. Do not edit by hand: run the tool.",
        "`tools/bfs_cards.py --check` (also run by `cargo test -p brainos-nostd-check`)",
        "fails if this file or any file's card is out of date.",
        "",
        "## How to read a file",
        "",
        "Every Rust file opens with the same five lines:",
        "",
        "| line | answers | maintained by |",
        "|---|---|---|",
        "| `LEVEL` | which domain or stage of Architecture v1.3 this belongs to, and its crate | the table in the tool |",
        "| `PURPOSE` | what the file is for, in one line | a human; preserved by the tool |",
        "| `DEFINES` | the public items it declares | computed |",
        "| `USES` | what it imports: this crate, other BrAInOS crates, external crates | computed |",
        "| `USED BY` | which files import it | computed |",
        "",
        "Below the card, the file's own long-form doc comment explains the why.",
        "",
        "## The levels, bottom to top",
        "",
        "Authority flows down only through Domain 3. Information flows up freely.",
        "",
    ]
    by_level = {}
    for rel, i in sorted(info.items()):
        by_level.setdefault(level_for(rel), []).append((rel, i))
    order = [l for _, l in LEVELS]
    seen = []
    for l in order:
        if l in by_level and l not in seen:
            seen.append(l)
    for l in by_level:
        if l not in seen:
            seen.append(l)
    for lvl in seen:
        files = by_level[lvl]
        L += ["### " + lvl, "", "| file | purpose | defines | uses | used by |", "|---|---|---|---|---|"]
        for rel, i in files:
            esc = lambda s: s.replace("|", "\\|")
            L.append(f"| `{rel}` | {esc(i['purpose'])} | {esc(i['defines'])} | {esc(i['uses'])} | {esc(i['used_by'])} |")
        L.append("")
    L += [
        "## Python, C and shell sources",
        "",
        "Listed here only. These files do not carry a card, because several are frozen",
        "by content hash for cluster campaigns and a comment edit would change the hash.",
        "",
        "| file | level | purpose | defines |",
        "|---|---|---|---|",
    ]
    for rel, lvl, purpose, defs in others():
        L.append(f"| `{rel}` | {lvl} | {purpose.replace('|', chr(92) + '|')} | {defs} |")
    todo = sorted(r for r, i in info.items() if i["purpose"].startswith("(not yet"))
    L += ["", "## Files still needing a PURPOSE line", ""]
    L += [f"- `{r}`" for r in todo] if todo else ["None."]
    return "\n".join(L) + "\n"


def main():
    check = "--check" in sys.argv
    info = build()
    stale = [r for r, i in info.items() if i["new"] != i["text"]]
    map_path = os.path.join(ROOT, "docs", "CODE_MAP.md")
    new_map = render_map(info)
    old_map = open(map_path).read() if os.path.exists(map_path) else ""
    if check:
        if stale or new_map != old_map:
            for r in stale[:20]:
                print("stale card:", r)
            if new_map != old_map:
                print("stale: docs/CODE_MAP.md")
            print("run tools/bfs_cards.py")
            sys.exit(1)
        print(f"bfs cards current: {len(info)} Rust files")
        return
    for r in stale:
        open(os.path.join(ROOT, r), "w").write(info[r]["new"])
    if new_map != old_map:
        open(map_path, "w").write(new_map)
    print(f"{len(info)} Rust files, {len(stale)} cards rewritten, map {'updated' if new_map != old_map else 'unchanged'}")


if __name__ == "__main__":
    main()
