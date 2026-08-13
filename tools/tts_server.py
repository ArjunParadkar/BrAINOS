#!/usr/bin/env python3
"""
tts_server.py — the entity's voicebox, as its own organ process.

Chatterbox (Resemble AI) is the most natural local TTS we can run, but it
pins its own dependency world, so it lives in a dedicated venv
(tools/tts-env) and talks to the body daemon over a unix socket. One line
of text in, speech synthesized and played on the host speakers, "DONE"
back when the room is quiet again.

    tools/tts-env/bin/python tools/tts_server.py --socket /tmp/voicebox.sock

Protocol (newline-framed): request is a text line; response is
"DONE\n" (played) or "ERR\n". The daemon holds its mic deafness window
open until DONE arrives.
"""

import argparse
import os
import socket
import subprocess
import sys
import time

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", required=True)
    ap.add_argument("--exaggeration", type=float, default=0.45,
                    help="chatterbox emotion intensity (0..1)")
    ap.add_argument("--cfg-weight", type=float, default=0.5)
    args = ap.parse_args()

    print("[voicebox] loading chatterbox ...", flush=True)
    import soundfile as sf
    import torch
    from chatterbox.tts import ChatterboxTTS

    model = ChatterboxTTS.from_pretrained(
        device="cuda" if torch.cuda.is_available() else "cpu"
    )
    # warm the kernels so the first real sentence isn't slow
    model.generate("warm up.")
    print(f"[voicebox] ready ({torch.cuda.memory_allocated()/2**30:.1f} GiB vram)",
          flush=True)

    if os.path.exists(args.socket):
        os.unlink(args.socket)
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(args.socket)
    srv.listen(1)
    print(f"[voicebox] listening on {args.socket}", flush=True)

    while True:
        conn, _ = srv.accept()
        f = conn.makefile("rwb")
        try:
            for line in f:
                text = line.decode("utf-8", "ignore").strip()
                if not text:
                    continue
                t0 = time.time()
                try:
                    wav = model.generate(
                        text,
                        exaggeration=args.exaggeration,
                        cfg_weight=args.cfg_weight,
                    )
                    path = f"/tmp/voicebox_{os.getpid()}.wav"
                    sf.write(path, wav.squeeze(0).cpu().numpy(), model.sr)
                    print(f"[voicebox] {wav.shape[1]/model.sr:.1f}s in "
                          f"{time.time()-t0:.1f}s: {text[:60]}", flush=True)
                    subprocess.run(["pw-play", path], capture_output=True, timeout=180)
                    os.unlink(path)
                    f.write(b"DONE\n")
                except Exception as e:
                    print(f"[voicebox] failed: {e!r}", flush=True)
                    f.write(b"ERR\n")
                f.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            conn.close()
            print("[voicebox] daemon disconnected; waiting", flush=True)


if __name__ == "__main__":
    main()
