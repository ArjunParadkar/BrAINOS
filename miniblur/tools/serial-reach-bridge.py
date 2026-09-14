#!/usr/bin/env python3
"""Bench link for Mini-Blur board A (Execution Plan §4.4): relay the board's
USB-Serial-JTAG line to the seat's Reach port, both ways, unchanged.

The board speaks the Reach wire protocol as newline-framed JSON on its USB
serial port; the seat listens for Reach bodies on TCP. This bridge is a
wire, nothing more — it never parses, never stores, never signs. To the
operator, a board behind this bridge and `miniblur-sim` are the same body.

    python3 miniblur/tools/serial-reach-bridge.py --port /dev/ttyACM0 --seat 192.168.1.10:7777

Standard library only (termios, no pyserial). Reconnects to the seat when it
goes away; the board re-hails on its own.
"""
import argparse
import os
import select
import socket
import sys
import termios
import time


def open_port(path, baud=115200):
    fd = os.open(path, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    attrs = termios.tcgetattr(fd)
    # raw mode, 8N1, no flow control; USB-CDC ignores the baud but termios wants one
    attrs[0] = 0  # iflag
    attrs[1] = 0  # oflag
    attrs[2] = termios.CS8 | termios.CREAD | termios.CLOCAL  # cflag
    attrs[3] = 0  # lflag
    b = getattr(termios, f"B{baud}", termios.B115200)
    attrs[4] = b
    attrs[5] = b
    attrs[6][termios.VMIN] = 0
    attrs[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
    return fd


def connect(seat, timeout=5.0):
    host, port = seat.rsplit(":", 1)
    s = socket.create_connection((host, int(port)), timeout=timeout)
    s.setblocking(False)
    s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--seat", required=True, help="host:port of the seat's Reach listener")
    ap.add_argument("--echo", action="store_true", help="print every frame to stderr")
    a = ap.parse_args()

    fd = open_port(a.port)
    print(f"[bridge] {a.port} <-> {a.seat}", file=sys.stderr)
    sock = None
    to_seat = b""
    to_board = b""
    while True:
        if sock is None:
            try:
                sock = connect(a.seat)
                print("[bridge] seat connected", file=sys.stderr)
            except OSError as e:
                print(f"[bridge] seat: {e}; retrying", file=sys.stderr)
                time.sleep(2)
                continue
        rl = [fd, sock]
        wl = []
        if to_seat:
            wl.append(sock)
        if to_board:
            wl.append(fd)
        r, w, _ = select.select(rl, wl, [], 0.05)
        try:
            if fd in r:
                chunk = os.read(fd, 4096)
                if chunk:
                    to_seat += chunk
                    if a.echo:
                        for l in chunk.splitlines():
                            print(f"  board> {l[:160].decode(errors='replace')}", file=sys.stderr)
            if sock in r:
                chunk = sock.recv(4096)
                if not chunk:
                    raise ConnectionError("seat closed")
                to_board += chunk
                if a.echo:
                    for l in chunk.splitlines():
                        print(f"  seat > {l[:160].decode(errors='replace')}", file=sys.stderr)
            if sock in w and to_seat:
                n = sock.send(to_seat)
                to_seat = to_seat[n:]
            if fd in w and to_board:
                n = os.write(fd, to_board)
                to_board = to_board[n:]
        except (OSError, ConnectionError) as e:
            print(f"[bridge] seat link dropped: {e}", file=sys.stderr)
            try:
                sock.close()
            except OSError:
                pass
            sock = None
            to_seat = b""
            to_board = b""
            time.sleep(1)


if __name__ == "__main__":
    main()
