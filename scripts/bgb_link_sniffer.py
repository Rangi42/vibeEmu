#!/usr/bin/env python3
"""
BGB Link Cable Protocol Sniffer

This script acts as a TCP proxy between two BGB emulator instances,
logging all packets in a human-readable format for analysis.

Usage:
    1. Run this script: python bgb_link_sniffer.py [listen_port] [forward_host] [forward_port]
    2. In BGB #1: Link -> Listen, use a different port (e.g., 5001)
    3. In BGB #2: Link -> Connect to localhost:5000 (this script's listen port)

The script will forward traffic between the two instances while logging everything.

Default: Listen on 5000, forward to localhost:5001
"""

import argparse
import signal
import socket
import struct
import sys
import threading
from datetime import datetime

# BGB protocol commands
COMMANDS = {
    1: "VERSION",
    101: "JOYPAD",
    104: "SYNC1",
    105: "SYNC2",
    106: "SYNC3",
    108: "STATUS",
    109: "WANTDISCONNECT",
}

STATUS_FLAGS = {
    0x01: "RUNNING",
    0x02: "PAUSED",
    0x04: "SUPPORT_RECONNECT",
}


def decode_packet(data: bytes) -> dict:
    """Decode an 8-byte BGB packet."""
    if len(data) < 8:
        return {"error": f"Short packet: {len(data)} bytes", "raw": data.hex()}

    b1, b2, b3, b4 = data[0], data[1], data[2], data[3]
    i1 = struct.unpack("<I", data[4:8])[0]

    result = {
        "cmd": b1,
        "cmd_name": COMMANDS.get(b1, f"UNKNOWN({b1})"),
        "b2": b2,
        "b3": b3,
        "b4": b4,
        "i1": i1,
        "raw": data.hex(),
    }

    # Add command-specific interpretation
    if b1 == 1:  # VERSION
        result["version"] = f"{b2}.{b3}.{b4}"
    elif b1 == 104:  # SYNC1
        result["data"] = f"0x{b2:02X}"
        result["control"] = f"0x{b3:02X}"
        result["timestamp"] = i1
        result["is_master"] = (b3 & 0x01) != 0
        result["high_speed"] = (b3 & 0x02) != 0
        result["double_speed"] = (b3 & 0x04) != 0
    elif b1 == 105:  # SYNC2
        result["data"] = f"0x{b2:02X}"
        result["control"] = f"0x{b3:02X}"
    elif b1 == 106:  # SYNC3
        if b2 == 0 and i1 != 0:
            result["type"] = "timestamp_sync"
            result["timestamp"] = i1
        elif b2 == 1:
            result["type"] = "ack"
        else:
            result["type"] = "unknown"
    elif b1 == 108:  # STATUS
        flags = []
        for flag_val, flag_name in STATUS_FLAGS.items():
            if b2 & flag_val:
                flags.append(flag_name)
        result["flags"] = flags if flags else ["NONE"]

    return result


def format_packet(packet: dict, direction: str) -> str:
    """Format a decoded packet for display."""
    timestamp = datetime.now().strftime("%H:%M:%S.%f")[:-3]
    arrow = "->" if direction == "CLIENT" else "<-"

    cmd_name = packet.get("cmd_name", "?")
    details = []

    if "version" in packet:
        details.append(f"ver={packet['version']}")
    if "data" in packet:
        details.append(f"data={packet['data']}")
    if "control" in packet:
        details.append(f"ctrl={packet['control']}")
    if "timestamp" in packet:
        details.append(f"ts={packet['timestamp']}")
    if "is_master" in packet:
        details.append("MASTER" if packet["is_master"] else "SLAVE")
    if "type" in packet:
        details.append(f"type={packet['type']}")
    if "flags" in packet:
        details.append(f"flags={','.join(packet['flags'])}")

    detail_str = " " + " ".join(details) if details else ""
    raw = packet.get("raw", "")

    return f"[{timestamp}] {direction:6} {arrow} {cmd_name:15}{detail_str}  [{raw}]"


class LinkSniffer:
    def __init__(
        self,
        listen_port: int,
        forward_host: str,
        forward_port: int,
        out_file: str | None,
    ):
        self.listen_port = listen_port
        self.forward_host = forward_host
        self.forward_port = forward_port
        self.running = True
        self._server_socket: socket.socket | None = None
        self.log_file = None
        self.out_file = out_file
        self._log_lock = threading.Lock()
        self._log_lines_since_flush = 0

    def stop(self):
        """Request all threads to stop and unblock blocking I/O."""
        self.running = False
        if self._server_socket is not None:
            try:
                self._server_socket.close()
            except OSError:
                pass

    def start(self):
        log_filename = (
            self.out_file
            if self.out_file
            else f"bgb_trace_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"
        )
        self.log_file = open(log_filename, "w", encoding="utf-8")
        print(f"Logging to {log_filename}")

        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server_socket = server
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("0.0.0.0", self.listen_port))
        server.listen(1)
        # Use a timeout so Ctrl-C/shutdown can break out of accept().
        server.settimeout(0.5)

        print(f"Listening on port {self.listen_port}")
        print(f"Will forward to {self.forward_host}:{self.forward_port}")
        print("Waiting for connection...")

        while self.running:
            try:
                client_sock, addr = server.accept()
            except socket.timeout:
                continue
            except OSError:
                # Socket closed during shutdown.
                break

            print(f"Client connected from {addr}")
            self.handle_connection(client_sock)

        try:
            server.close()
        except OSError:
            pass
        if self.log_file:
            self.log_file.close()

    def log(self, message: str):
        with self._log_lock:
            print(message)
            if self.log_file:
                self.log_file.write(message + "\n")
                self._log_lines_since_flush += 1
                if (
                    self._log_lines_since_flush >= 128
                    or message.startswith("Connection")
                    or message.startswith("Forward error")
                    or message.startswith("Failed to connect")
                ):
                    self.log_file.flush()
                    self._log_lines_since_flush = 0

    def handle_connection(self, client_sock: socket.socket):
        try:
            server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server_sock.connect((self.forward_host, self.forward_port))
            self.log(f"Connected to server at {self.forward_host}:{self.forward_port}")
        except Exception as e:
            self.log(f"Failed to connect to server: {e}")
            client_sock.close()
            return

        client_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        server_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        # Use timeouts so forwarding threads can exit promptly on shutdown.
        client_sock.settimeout(0.5)
        server_sock.settimeout(0.5)

        # Start forwarding threads
        client_to_server = threading.Thread(
            target=self.forward,
            args=(client_sock, server_sock, "CLIENT"),
            daemon=True,
        )
        server_to_client = threading.Thread(
            target=self.forward,
            args=(server_sock, client_sock, "SERVER"),
            daemon=True,
        )

        client_to_server.start()
        server_to_client.start()

        client_to_server.join()
        server_to_client.join()

        client_sock.close()
        server_sock.close()
        self.log("Connection closed")

    def forward(self, src: socket.socket, dst: socket.socket, direction: str):
        buffer = b""
        try:
            while self.running:
                try:
                    data = src.recv(4096)
                except socket.timeout:
                    continue
                if not data:
                    break

                # Forward first to avoid proxy-induced timing skew under heavy traffic.
                dst.sendall(data)
                buffer += data

                # Process complete 8-byte packets
                while len(buffer) >= 8:
                    packet_data = buffer[:8]
                    buffer = buffer[8:]

                    packet = decode_packet(packet_data)
                    self.log(format_packet(packet, direction))
        except Exception as e:
            self.log(f"Forward error ({direction}): {e}")


def main():
    parser = argparse.ArgumentParser(
        description="TCP proxy/sniffer for the BGB link protocol (8-byte packets)."
    )
    parser.add_argument(
        "listen_port",
        nargs="?",
        type=int,
        default=5000,
        help="Port to listen on (default: 5000)",
    )
    parser.add_argument(
        "forward_host",
        nargs="?",
        default="127.0.0.1",
        help="Host to forward to (default: 127.0.0.1)",
    )
    parser.add_argument(
        "forward_port",
        nargs="?",
        type=int,
        default=5001,
        help="Port to forward to (default: 5001)",
    )
    parser.add_argument(
        "--out",
        dest="out_file",
        default=None,
        help="Optional log file path. Defaults to bgb_trace_YYYYmmdd_HHMMSS.log",
    )

    args = parser.parse_args()
    listen_port = args.listen_port
    forward_host = args.forward_host
    forward_port = args.forward_port

    print("BGB Link Cable Protocol Sniffer")
    print("================================")
    print()
    print("Instructions:")
    print(f"  1. Start BGB #1: Link -> Listen on port {forward_port}")
    print(f"  2. Run this script (listening on {listen_port})")
    print(f"  3. Start BGB #2: Link -> Connect to localhost:{listen_port}")
    print()

    sniffer = LinkSniffer(listen_port, forward_host, forward_port, args.out_file)

    def _handle_sigint(_signum, _frame):
        sniffer.stop()

    signal.signal(signal.SIGINT, _handle_sigint)

    try:
        sniffer.start()
    except KeyboardInterrupt:
        sniffer.stop()
        print("\nShutting down...")


if __name__ == "__main__":
    main()
