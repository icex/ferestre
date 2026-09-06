#!/usr/bin/env python3
"""A scriptable WebSocket server, for reproducing libHttpClient's WebSocket
teardown without the game.

Minecraft's server-list join opens two WebSockets (Xbox Live's Real-Time
Activity socket and the multiplayer signalling socket). Something in that path
leaves an HC_WEBSOCKET_OBSERVER alive after its WebSocket has been destroyed,
and the game then deadlocks on the freed mutex. Driving libHttpClient against a
server we control turns a several-minute manual reproduction into a few seconds.

Behaviours (first argument):
  accept     complete the handshake and echo whatever arrives
  refuse     answer the upgrade with 403 and close
  drop       accept the TCP connection, then close it without answering
  half       send the handshake, then close the TCP connection immediately
  stall      complete the handshake, then never send anything
  closeframe complete the handshake, then send a Close frame
  flood      complete the handshake, then send many large frames

Usage: ws_server.py <behaviour> [port]
"""
import base64, hashlib, socket, struct, sys, threading, time

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def handshake(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(4096)
        if not chunk:
            return None
        data += chunk
    key = None
    for line in data.split(b"\r\n"):
        if line.lower().startswith(b"sec-websocket-key:"):
            key = line.split(b":", 1)[1].strip().decode()
    return key


def accept_response(key):
    accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
    return ("HTTP/1.1 101 Switching Protocols\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Accept: {accept}\r\n\r\n").encode()


def frame(payload, opcode=0x1):
    header = bytes([0x80 | opcode])
    n = len(payload)
    if n < 126:
        header += bytes([n])
    elif n < 1 << 16:
        header += bytes([126]) + struct.pack(">H", n)
    else:
        header += bytes([127]) + struct.pack(">Q", n)
    return header + payload


def read_frame(conn):
    hdr = conn.recv(2)
    if len(hdr) < 2:
        return None
    masked, n = hdr[1] & 0x80, hdr[1] & 0x7F
    if n == 126:
        n = struct.unpack(">H", conn.recv(2))[0]
    elif n == 127:
        n = struct.unpack(">Q", conn.recv(8))[0]
    mask = conn.recv(4) if masked else b""
    body = b""
    while len(body) < n:
        part = conn.recv(n - len(body))
        if not part:
            break
        body += part
    if masked:
        body = bytes(b ^ mask[i % 4] for i, b in enumerate(body))
    return hdr[0] & 0x0F, body


def serve(conn, behaviour):
    try:
        if behaviour == "drop":
            conn.close()
            return
        key = handshake(conn)
        if key is None:
            return
        if behaviour == "refuse":
            conn.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            conn.close()
            return
        conn.sendall(accept_response(key))
        if behaviour == "half":
            conn.close()
            return
        if behaviour == "closeframe":
            conn.sendall(frame(struct.pack(">H", 1001) + b"going away", opcode=0x8))
            time.sleep(0.2)
            conn.close()
            return
        if behaviour == "flood":
            for i in range(64):
                conn.sendall(frame(b"x" * 60000))
            time.sleep(1)
            conn.close()
            return
        if behaviour == "stall":
            time.sleep(120)
            return
        while True:                                   # accept: echo
            got = read_frame(conn)
            if got is None:
                break
            op, body = got
            if op == 0x8:
                conn.sendall(frame(b"", opcode=0x8))
                break
            conn.sendall(frame(body, opcode=op if op in (0x1, 0x2) else 0x1))
    except (OSError, struct.error):
        pass
    finally:
        try:
            conn.close()
        except OSError:
            pass


def main():
    behaviour = sys.argv[1] if len(sys.argv) > 1 else "accept"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 18080
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(16)
    print(f"ws://127.0.0.1:{port} behaviour={behaviour}", flush=True)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=serve, args=(conn, behaviour), daemon=True).start()


if __name__ == "__main__":
    main()
