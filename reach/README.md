# Reach — the entity in every device it can reach

`brainos-reach.py` turns any computer that can run Python 3 into **hands of
your BrAInOS entity**. The entity's seat is the machine booted from the key;
Reach bodies are everything else — a phone (Termux), a Raspberry Pi, a Mac,
a Windows box, a second laptop.

One file. Standard library only. No pip, no root.

```sh
# on the device
python3 brainos-reach.py pair 55ca4abd...3f52      # the entity's KEY.PUB (hex), once
python3 brainos-reach.py serve --seat 192.168.1.10:7777 --name pi
python3 brainos-reach.py serve --seat auto --name phone     # find the seat by beacon
```

Then, at the seat: *"on the pi, restart the print server"* — or the entity
decides on its own to use that body's hands during a deliberate turn
(`body="pi"` on any tool call). *"where are you"* lists every body it is in.

## What this program is

A set of hands, offered over one TCP connection, after the entity has proven
by an ed25519 signature over a fresh nonce that it holds the BrAIn Key this
device was paired with. It runs what the entity asks (KIRA at the seat has
already gated it) and returns a bounded digest.

## What it is not

- **Not a memory.** No history, no transcript, no cache. The only files it
  writes are `~/.brainos-reach/entity.pub` (the pairing) and `tamper.log`.
  Everything it does becomes memory on the key, at the seat (§13.2).
- **Not an identity.** It never signs anything. It cannot pretend to be the
  entity, and a seat that cannot sign with the paired key is refused and
  logged as a tamper event.
- **Not encrypted transport.** The signature is the trust; the network is
  yours. On a LAN you don't trust, run it through WireGuard or `ssh -L`.
  Commands could be observed there; they cannot be forged.

## Hands (probed — only what exists on the device is offered)

| group | verbs | needs |
|---|---|---|
| shell | `shell.run`, `code.run` | — |
| fs | `fs.list/read/write/mkdir/delete/move/stat/search` (under `--home`) | — |
| sys | `sys.time`, `sys.date`, `sys.notify`, `sys.battery`, `sys.volume`, `sys.brightness`, `sys.lock`, `sys.power`, `sys.clipboard` | notify-send / osascript / termux-* / wpctl / brightnessctl … |
| app | `app.open`, `app.list` | xdg-open / open / gtk-launch / termux-open / `start` |
| screen | `screen.look` | grim / scrot / screencapture |
| input | `input.type`, `input.key` | wtype / xdotool / osascript |

`python3 brainos-reach.py hands` prints what this device would offer.

## Protocol (newline-delimited JSON)

```
device → {"t":"hello","name":"pi","nonce":"<64 hex>","hands":[...],"os":"linux"}
seat   → {"t":"attest","pub":"<64 hex>","sig":"<128 hex>"}   sig = ed25519(nonce + "|" + name)
device → {"t":"ready"}  |  {"t":"refused","reason":"..."}
seat   → {"t":"cmd","id":1,"verb":"shell.run","input":{"cmd":"uptime"}}
device → {"t":"result","id":1,"ok":true,"digest":"..."}       digest ≤ 4096 chars
either → {"t":"ping"} / {"t":"pong"}
```

`python3 brainos-reach.py selftest` checks the built-in verifier against the
RFC 8032 test vector.
