#!/usr/bin/env python3
"""Raw IPC client for the daemon socket (stdlib only).

  ipc.py request JSON            -> sends one request (without "v") and prints the response line
  ipc.py subscribe SECONDS [JSON] -> subscribes, optionally sends JSON requests on a second connection after
                                     the first event, and prints every line received within SECONDS
  ipc.py stall ID                -> subscribes without reading while the overlay ID is moved transiently many
                                    times, then prints "closed" if the daemon dropped the subscriber
"""
import json
import os
import select
import socket
import sys
import time


def connect():
    sock = socket.socket(socket.AF_UNIX)
    sock.connect(os.path.join(os.environ["XDG_RUNTIME_DIR"], "aniway.sock"))
    return sock


def send(sock, request):
    sock.sendall((json.dumps({"v": 1, **request}) + "\n").encode())


def read_lines(sock, seconds):
    buf, end = b"", time.time() + seconds
    while time.time() < end:
        if select.select([sock], [], [], max(end - time.time(), 0))[0]:
            data = sock.recv(65536)
            if not data:
                break
            buf += data
    return [line for line in buf.decode().split("\n") if line]


def stall(overlay):
    sock = connect()
    send(sock, {"cmd": "subscribe"})
    for x in range(120):
        other = connect()
        send(other, {"cmd": "set", "id": overlay, "patch": {"x": x}, "transient": True})
        read_lines(other, 0.05)
    sock.settimeout(2)
    try:
        while sock.recv(1 << 20):
            pass
        print("closed")
    except socket.timeout:
        print("open")


def main():
    if sys.argv[1] == "stall":
        return stall(sys.argv[2])
    if sys.argv[1] == "request":
        sock = connect()
        send(sock, json.loads(sys.argv[2]))
        print(read_lines(sock, 1)[0])
        return
    sock = connect()
    send(sock, {"cmd": "subscribe"})
    seconds = float(sys.argv[2])
    lines = read_lines(sock, 0.5)
    for request in sys.argv[3:]:
        other = connect()
        send(other, json.loads(request))
        read_lines(other, 0.3)
    lines += read_lines(sock, seconds)
    print("\n".join(lines))


main()
