#!/usr/bin/env python3
"""SOCKS5 allowlist gate for the forge-bench lane (runs on hbox).

Listens on 127.0.0.1:7778 and forwards SOCKS5 CONNECT traffic ONLY to the
loopback parano1d devnet RPC (127.0.0.1:21241). Every other target is
refused and logged, so the log doubles as the lane's egress audit trail:
forge's embedded dev-fee pool attempts (stratum+ssl://eu2.innovlab.cc:19601)
appear there as refused CONNECTs and never leave the box.

forge is started with --proxy socks5://127.0.0.1:7778; its README states
every pool connection (user pool, backups and dev fee) goes through it.
"""

import asyncio
import struct
import time

LISTEN_HOST = "127.0.0.1"
LISTEN_PORT = 7778
ALLOWED = {("127.0.0.1", 21241)}
LOG_PATH = "/home/hbox/lanes/forge-bench/logs/gate.log"

REFUSED = {}
ACCEPTED = 0


def log(message: str) -> None:
    stamp = time.strftime("%Y-%m-%d %H:%M:%S")
    with open(LOG_PATH, "a", encoding="utf-8") as handle:
        handle.write(f"{stamp} {message}\n")


async def pump(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        while True:
            chunk = await reader.read(65536)
            if not chunk:
                break
            writer.write(chunk)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        try:
            writer.close()
        except OSError:
            pass


async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    global ACCEPTED
    peer = writer.get_extra_info("peername")
    try:
        head = await reader.readexactly(2)
        methods = await reader.readexactly(head[1])
        if 0x00 not in methods:
            writer.write(b"\x05\xff")
            await writer.drain()
            writer.close()
            log(f"{peer} no acceptable auth method {methods.hex()}")
            return
        writer.write(b"\x05\x00")
        await writer.drain()

        hdr = await reader.readexactly(4)
        version, cmd, _, atyp = hdr
        if version != 5 or cmd != 0x01:
            writer.write(b"\x05\x07\x00\x01\x00\x00\x00\x00\x00\x00")
            await writer.drain()
            writer.close()
            log(f"{peer} non-CONNECT request v{version} cmd={cmd}")
            return

        if atyp == 0x01:
            raw = await reader.readexactly(4)
            host = ".".join(str(b) for b in raw)
        elif atyp == 0x03:
            length = (await reader.readexactly(1))[0]
            host = (await reader.readexactly(length)).decode("utf-8", "replace")
        elif atyp == 0x04:
            raw = await reader.readexactly(16)
            host = ":".join(raw[i : i + 2].hex() for i in range(0, 16, 2))
        else:
            writer.write(b"\x05\x08\x00\x01\x00\x00\x00\x00\x00\x00")
            await writer.drain()
            writer.close()
            log(f"{peer} bad ATYP {atyp}")
            return
        port = struct.unpack(">H", await reader.readexactly(2))[0]

        key = (host, port)
        if key not in ALLOWED:
            REFUSED[key] = REFUSED.get(key, 0) + 1
            log(f"{peer} REFUSED CONNECT {host}:{port} (count={REFUSED[key]})")
            writer.write(b"\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00")
            await writer.drain()
            writer.close()
            return

        try:
            upstream_reader, upstream_writer = await asyncio.wait_for(
                asyncio.open_connection(host, port), timeout=10
            )
        except (OSError, asyncio.TimeoutError) as exc:
            log(f"{peer} CONNECT {host}:{port} upstream error: {exc}")
            writer.write(b"\x05\x01\x00\x01\x00\x00\x00\x00\x00\x00")
            await writer.drain()
            writer.close()
            return

        ACCEPTED += 1
        writer.write(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
        await writer.drain()
        await asyncio.gather(pump(reader, upstream_writer), pump(upstream_reader, writer))
    except (asyncio.IncompleteReadError, ConnectionError, OSError) as exc:
        log(f"{peer} session error: {exc!r}")
    finally:
        try:
            writer.close()
        except OSError:
            pass


async def main() -> None:
    log(f"gate up on {LISTEN_HOST}:{LISTEN_PORT}; allowed targets: {sorted(ALLOWED)}")
    server = await asyncio.start_server(handle, LISTEN_HOST, LISTEN_PORT)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
