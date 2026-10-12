#!/usr/bin/env python3
"""Send one TSL UMD 5.0 display message (a test tally source).

    tsl_send.py udp|tcp HOST PORT SCREEN INDEX LH RH LABEL

LH and RH are lamp values 0-3 (0 off, 1 red, 2 green, 3 amber). The label goes as
UTF-16LE (the unicode flag). TCP wraps the packet in DLE/STX with DLE doubled.
"""
import socket
import struct
import sys


def packet(screen, index, lh, rh, label):
    text = label.encode("utf-16-le")
    control = (rh & 3) | ((lh & 3) << 4) | (3 << 6)
    body = struct.pack("<BBH", 0, 1, screen)  # version 0, flags: unicode
    body += struct.pack("<HHH", index, control, len(text)) + text
    return struct.pack("<H", len(body)) + body


def main():
    proto, host, port, screen, index, lh, rh, label = sys.argv[1:9]
    p = packet(int(screen), int(index), int(lh), int(rh), label)
    if proto == "udp":
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.sendto(p, (host, int(port)))
    else:
        framed = b"\xfe\x02" + p.replace(b"\xfe", b"\xfe\xfe")
        with socket.create_connection((host, int(port)), timeout=2) as s:
            s.sendall(framed)


if __name__ == "__main__":
    main()
