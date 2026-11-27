#!/usr/bin/env python3
"""A fake ESP32-C5 multi-node board, as C5MK presents itself to RotorHazard.

Eight seats on one serial port, node API 36, the standard RotorHazard node
protocol: revision handshake, multinode count, slot switching, frequency
write/read-back and the API 36 RX register test.

It creates a pty pair, writes the slave path to --port-file and serves the
master side until killed, recording what the server asked for in --log
(JSON, written on every change so a test can read it while running).

Usage:
    fake_c5mk.py --port-file /tmp/port --log /tmp/board.json [--seats 8]
"""

import argparse
import json
import os
import select
import sys
import time
import tty

START = time.time()

READ_REVISION_CODE = 0x22
READ_FREQUENCY = 0x03
TEST_RX_REGISTER = 0x04
READ_LAP_STATS = 0x05
READ_LAP_PASS_STATS = 0x0D
READ_LAP_EXTREMUMS = 0x0E
READ_RHFEAT_FLAGS = 0x11
READ_NODE_RSSI_PEAK = 0x23
READ_NODE_RSSI_NADIR = 0x24
READ_ENTER_AT_LEVEL = 0x31
READ_EXIT_AT_LEVEL = 0x32
READ_TIME_MILLIS = 0x33
READ_MULTINODE_COUNT = 0x39
READ_FW_VERSION = 0x3D
READ_FW_PROCTYPE = 0x40

WRITE_FREQUENCY = 0x51
WRITE_ENTER_AT_LEVEL = 0x71
WRITE_EXIT_AT_LEVEL = 0x72
WRITE_CURNODE_INDEX = 0x7A

# reply sizes the server asks for (serial.rs read_block sizes)
READ_SIZES = {
    READ_REVISION_CODE: 2,
    READ_FREQUENCY: 2,
    TEST_RX_REGISTER: 1,
    READ_LAP_STATS: 28,
    READ_LAP_PASS_STATS: 8,
    READ_LAP_EXTREMUMS: 8,
    READ_RHFEAT_FLAGS: 2,
    READ_NODE_RSSI_PEAK: 1,
    READ_NODE_RSSI_NADIR: 1,
    READ_ENTER_AT_LEVEL: 1,
    READ_EXIT_AT_LEVEL: 1,
    READ_TIME_MILLIS: 4,
    READ_MULTINODE_COUNT: 1,
    READ_FW_VERSION: 16,
    READ_FW_PROCTYPE: 16,
}
WRITE_SIZES = {
    WRITE_FREQUENCY: 2,
    WRITE_ENTER_AT_LEVEL: 1,
    WRITE_EXIT_AT_LEVEL: 1,
}

SUPPORTED_MAX = 5917
SUPPORTED_MIN = 5180


def checksum(data):
    return sum(data) & 0xFF


class Board:
    def __init__(self, seats=8, api=36, unsupported=()):
        self.seats = seats
        self.api = api
        # frequencies this fake RX cannot tune, used to prove the register test
        self.unsupported = set(unsupported)
        self.frequency = [0] * seats
        self.enter_at = [90] * seats
        self.exit_at = [80] * seats
        self.slot = 0
        self.log = {
            "api": api,
            "seats": seats,
            "slot_switches": 0,
            "freq_writes": [0] * seats,
            "freq_values": [0] * seats,
            "register_tests": 0,
            "register_failures": 0,
            "register_failing_freq": 0,
            "power_downs": 0,
            "events": [],
        }

    # -- payloads ---------------------------------------------------------
    def _text16(self, text):
        raw = text.encode()[:16]
        return raw + b"\0" * (16 - len(raw))

    def supported(self, freq):
        return (
            freq != 0
            and SUPPORTED_MIN <= freq <= SUPPORTED_MAX
            and freq not in self.unsupported
        )

    def payload(self, command):
        size = READ_SIZES[command]
        if command == READ_REVISION_CODE:
            return bytes([0x25, self.api & 0xFF])
        if command == READ_MULTINODE_COUNT:
            return bytes([self.seats])
        if command == READ_RHFEAT_FLAGS:
            return bytes([0x00, 0x0F])
        if command == READ_FW_VERSION:
            return self._text16("1.2.2-C5MK-sim")
        if command == READ_FW_PROCTYPE:
            return self._text16("ESP32-C5")
        if command == READ_FREQUENCY:
            f = self.frequency[self.slot]
            return bytes([f >> 8, f & 0xFF])
        if command == TEST_RX_REGISTER:
            self.log["register_tests"] += 1
            ok = self.supported(self.frequency[self.slot])
            if not ok:
                self.log["register_failures"] += 1
                self.log["register_failing_freq"] = self.frequency[self.slot]
            return bytes([1 if ok else 0])
        if command == READ_ENTER_AT_LEVEL:
            return bytes([self.enter_at[self.slot]])
        if command == READ_EXIT_AT_LEVEL:
            return bytes([self.exit_at[self.slot]])
        if command == READ_NODE_RSSI_PEAK:
            return bytes([3])
        if command == READ_NODE_RSSI_NADIR:
            return bytes([1])
        # lap stats, time, anything else: zeros mean "no pass"
        return bytes(size)

    # -- writes -----------------------------------------------------------
    def apply(self, command, data):
        if command == WRITE_FREQUENCY:
            freq = (data[0] << 8) | data[1]
            if freq in (1111, 0):
                # RH powers a disabled seat down; the board keeps it off
                self.log["power_downs"] += 1
                self.frequency[self.slot] = 0
            else:
                self.frequency[self.slot] = freq
            self.log["freq_writes"][self.slot] += 1
            self.log["freq_values"][self.slot] = self.frequency[self.slot]
            self.log["events"].append(
                [round(time.time() - START, 2), self.slot, self.frequency[self.slot]]
            )
        elif command == WRITE_ENTER_AT_LEVEL:
            self.enter_at[self.slot] = data[0]
        elif command == WRITE_EXIT_AT_LEVEL:
            self.exit_at[self.slot] = data[0]


def serve(master, board, log_path):
    def flush():
        tmp = log_path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as handle:
            json.dump(board.log, handle)
        os.replace(tmp, log_path)

    flush()
    pending = bytearray()
    while True:
        ready, _, _ = select.select([master], [], [], 0.5)
        if not ready:
            continue
        chunk = os.read(master, 4096)
        if not chunk:
            return
        pending.extend(chunk)
        progressed = True
        while progressed and pending:
            progressed = False
            command = pending[0]
            if command in READ_SIZES:
                del pending[0]
                payload = board.payload(command)
                os.write(master, payload + bytes([checksum(payload)]))
                progressed = True
            elif command in WRITE_SIZES:
                length = WRITE_SIZES[command] + 1  # payload + checksum
                if len(pending) < 1 + length:
                    break
                payload = bytes(pending[1:1 + length - 1])
                del pending[:1 + length]
                board.apply(command, payload)
                progressed = True
            elif command == WRITE_CURNODE_INDEX:
                if len(pending) < 3:
                    break
                slot = pending[1]
                if slot < board.seats:
                    board.slot = slot
                    board.log["slot_switches"] += 1
                reply = bytes([board.slot])
                os.write(master, reply + bytes([checksum(reply)]))
                del pending[:3]
                progressed = True
            else:
                # unknown command: drop it so the stream cannot wedge
                del pending[0]
                progressed = True
        flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--seats", type=int, default=8)
    parser.add_argument("--api", type=int, default=36)
    parser.add_argument("--unsupported", type=int, action="append", default=[])
    args = parser.parse_args()

    master, slave = os.openpty()
    tty.setraw(master)
    tty.setraw(slave)
    slave_path = os.ttyname(slave)
    with open(args.port_file + ".tmp", "w", encoding="utf-8") as handle:
        handle.write(slave_path)
    os.replace(args.port_file + ".tmp", args.port_file)
    board = Board(seats=args.seats, api=args.api, unsupported=args.unsupported)
    try:
        serve(master, board, args.log)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
