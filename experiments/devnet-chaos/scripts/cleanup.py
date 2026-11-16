#!/usr/bin/env python3
"""Emergency/idle cleanup: stop exactly the PIDs recorded in run/*.pid.

Run this if a driver crashed: it reads every PID file still present, verifies
the command line looks like one of ours, and signals that PID only. Never
matches by pattern.
"""

import os
import signal
import subprocess
import sys
import time
from pathlib import Path

RUN = Path(__file__).resolve().parents[1] / "run"
MARKERS = ("parano1d", "anon-mempool-pin")


def pid_cmdline(pid):
    try:
        return subprocess.run(
            ["ps", "-p", str(pid), "-o", "command="], capture_output=True, text=True
        ).stdout.strip()
    except Exception:  # noqa: BLE001
        return ""


def main():
    pid_files = sorted(RUN.glob("*.pid"))
    if not pid_files:
        print("no PID files; nothing to stop")
        return 0
    for pid_file in pid_files:
        try:
            pid = int(pid_file.read_text().strip())
        except ValueError:
            print(f"[skip] {pid_file.name}: unparseable")
            continue
        cmdline = pid_cmdline(pid)
        if not cmdline:
            print(f"[gone] {pid_file.name}: pid {pid} not running")
            pid_file.unlink()
            continue
        if not any(marker in cmdline for marker in MARKERS) or "devnet-chaos" not in cmdline:
            print(f"[REFUSE] {pid_file.name}: pid {pid} cmdline is not ours: {cmdline[:120]}")
            continue
        print(f"[stop] {pid_file.name}: pid {pid} ({cmdline[:80]})")
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        time.sleep(0.3)
    time.sleep(3)
    for pid_file in pid_files:
        try:
            pid = int(pid_file.read_text().strip())
        except (ValueError, OSError):
            continue
        if pid_cmdline(pid):
            try:
                os.kill(pid, signal.SIGKILL)
                print(f"[kill] {pid_file.name}: pid {pid}")
            except ProcessLookupError:
                pass
        pid_file.unlink(missing_ok=True)
    print("cleanup complete")
    return 0


if __name__ == "__main__":
    sys.exit(main())
