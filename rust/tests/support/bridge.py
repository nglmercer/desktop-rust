"""Shared test helpers for bridge/CLI integration suites (phase 0 recovery).

Recovered from the deleted ``rust/tests/smoke.py`` (commit 22a04ffa49) and
moved here so the eight suites that imported ``BINARY`` / ``WebSocket``
continue to run against the current ``desktop-rs`` binary and loopback
WebSocket bridge without depending on the retired ``rust/web`` assets.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess

BINARY = Path(__file__).resolve().parents[2] / 'target/debug/desktop-rs'


def run(*args, **kwargs):
    return subprocess.run([str(BINARY), *args], capture_output=True, text=True, **kwargs)


def read_exact(sock, size):
    data = b''
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise EOFError('socket closed')
        data += chunk
    return data


class WebSocket:
    def __init__(self, authority, origin):
        host, port = authority.split(':')
        self.sock = socket.create_connection((host, int(port)), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f'GET /ws HTTP/1.1\r\nHost: {authority}\r\n'
                           f'Origin: {origin}\r\nUpgrade: websocket\r\n'
                           f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n'
                           'Sec-WebSocket-Version: 13\r\n\r\n').encode())
        header = b''
        while not header.endswith(b'\r\n\r\n'):
            header += read_exact(self.sock, 1)
        self.accepted = header.startswith(b'HTTP/1.1 101')
        if self.accepted:
            expected = base64.b64encode(hashlib.sha1(
                (key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
            assert expected in header
        else:
            assert b'403' in header

    def send(self, value):
        data = json.dumps(value).encode()
        mask = os.urandom(4)
        length = bytes([0x80 | len(data)]) if len(data) < 126 else bytes([0xfe]) + struct.pack('!H', len(data))
        self.sock.sendall(bytes([0x81]) + length + mask +
                          bytes(byte ^ mask[i % 4] for i, byte in enumerate(data)))

    def receive(self, events=False):
        first, second = read_exact(self.sock, 2)
        size = second & 127
        if size == 126:
            size = struct.unpack('!H', read_exact(self.sock, 2))[0]
        elif size == 127:
            size = struct.unpack('!Q', read_exact(self.sock, 8))[0]
        data = read_exact(self.sock, size)
        if first & 15 == 8:
            return None
        value = json.loads(data)
        if 'event' in value and not events:
            return self.receive(events=events)
        return value

    def close(self):
        self.sock.close()
