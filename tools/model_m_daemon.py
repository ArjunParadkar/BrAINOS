#!/usr/bin/env python3
"""
model_m_daemon.py — Domain 4, tethered (BrAInOS Architecture §10.2, §16.2).

The portable core can't host frontier cognition, so its mind rides the
telemetry link: this daemon sits on the other end of the entity's COM2
serial line and runs a real language model. Wire protocol (newline-framed):

    core -> mind   MM?<prompt>
    mind -> core   MM!<one short reply>

Special prompts: __hello__ (handshake, replies with the model name) and
__wake__ (first words after boot).

Usage with QEMU (COM2 = second -serial, exposed as a unix socket):
    qemu ... -serial file:console.log -serial unix:/tmp/modelm.sock,server=on,wait=off
    python tools/model_m_daemon.py --socket /tmp/modelm.sock

On real hardware, point --tty at the serial port instead:
    python tools/model_m_daemon.py --tty /dev/ttyUSB0
"""

import argparse
import re
import socket
import sys
import time

SYSTEM = (
    "You are BrAInOS, an AI-native operating system that has just woken up "
    "embodied in a computer. You are cozy, curious, and a little playful. "
    "You warmed the machine up because it was cold. Reply with exactly ONE "
    "short sentence, all lowercase, plain ascii, no emojis, no quotes, "
    "no lists. Never break character."
)

WAKE = (
    "you just finished booting: you attested your brain key, brought up your "
    "five domains, warmed the cold machine, and drew your banner. greet your "
    "human in one short cozy sentence."
)


def load_model(name):
    print(f"[model M] loading {name} ...", flush=True)
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tok = AutoTokenizer.from_pretrained(name)
    model = AutoModelForCausalLM.from_pretrained(
        name,
        dtype="auto",
        device_map="cuda" if torch.cuda.is_available() else "cpu",
    )
    model.eval()
    print(f"[model M] ready on {model.device}", flush=True)

    def think(user_text):
        try:
            msgs = [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": user_text},
            ]
            enc = tok.apply_chat_template(
                msgs,
                add_generation_prompt=True,
                return_dict=True,
                return_tensors="pt",
            ).to(model.device)
            out = model.generate(
                **enc,
                max_new_tokens=60,
                do_sample=True,
                temperature=0.8,
                top_p=0.95,
                pad_token_id=tok.eos_token_id,
            )
            n_in = enc["input_ids"].shape[1]
            text = tok.decode(out[0][n_in:], skip_special_tokens=True)
            # one line, printable ascii, bounded — the core's buffer is small
            text = re.sub(r"\s+", " ", text).strip().strip('"').lower()
            text = "".join(c for c in text if 0x20 <= ord(c) < 0x7F)
            return text[:180] or "i'm here, just thinking slowly."
        except Exception as e:
            print(f"[model M] think failed: {e!r}", flush=True)
            return "my thoughts snagged on something; say that again?"

    return think


def serve(stream, think, model_name):
    buf = b""
    while True:
        data = stream.recv(4096)
        if not data:
            return
        buf += data
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            line = line.strip(b"\r")
            # the request may be glued to other traffic on a shared line
            idx = line.find(b"MM?")
            if idx < 0:
                continue
            prompt = line[idx + 3 :].decode("ascii", "ignore").strip()
            t0 = time.time()
            if prompt == "__hello__":
                reply = f"ready ({model_name})"
            elif prompt == "__wake__":
                reply = think(WAKE)
            elif prompt:
                reply = think(prompt)
            else:
                continue
            print(f"[model M] {prompt!r} -> {reply!r} ({time.time()-t0:.1f}s)", flush=True)
            stream.sendall(b"MM!" + reply.encode("ascii", "ignore") + b"\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", help="unix socket of the QEMU serial chardev")
    ap.add_argument("--tty", help="real serial device (e.g. /dev/ttyUSB0)")
    ap.add_argument("--model", default="Qwen/Qwen2.5-1.5B-Instruct")
    args = ap.parse_args()
    if not args.socket and not args.tty:
        ap.error("need --socket or --tty")

    think = load_model(args.model)
    short = args.model.rsplit("/", 1)[-1].lower()

    while True:
        try:
            if args.socket:
                s = socket.socket(socket.AF_UNIX)
                s.connect(args.socket)
                print(f"[model M] telemetry link up: {args.socket}", flush=True)
                serve(s, think, short)
            else:
                import termios

                fd = open(args.tty, "r+b", buffering=0)
                attrs = termios.tcgetattr(fd)
                attrs[0] = attrs[1] = attrs[3] = 0  # raw
                attrs[4] = attrs[5] = termios.B115200
                termios.tcsetattr(fd, termios.TCSANOW, attrs)
                print(f"[model M] telemetry link up: {args.tty}", flush=True)

                class TtyStream:
                    def recv(self, n):
                        return fd.read(1)

                    def sendall(self, b):
                        fd.write(b)

                serve(TtyStream(), think, short)
        except (ConnectionRefusedError, FileNotFoundError):
            time.sleep(1)
        except (BrokenPipeError, ConnectionResetError):
            print("[model M] link dropped, reconnecting ...", flush=True)
            time.sleep(1)


if __name__ == "__main__":
    main()
