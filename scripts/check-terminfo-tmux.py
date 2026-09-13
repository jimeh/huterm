#!/usr/bin/env python3
"""Verify truecolor bytes through an isolated tmux client and outer PTY."""

import errno
import fcntl
import os
import pathlib
import pty
import select
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

RGB = b"\x1b[38;2;1;2;3m"
WITNESS = RGB + b"HUTERM_RGB"


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: check-terminfo-tmux.py TERMINFO_DIRECTORY")
    terminfo = pathlib.Path(sys.argv[1]).resolve()
    if not any((terminfo / bucket / "xterm-huterm").is_file() for bucket in ("x", "78")):
        raise SystemExit(f"xterm-huterm is missing from {terminfo}")

    with tempfile.TemporaryDirectory(prefix="huterm-tmux-") as state:
        home = pathlib.Path(state, "home")
        home.mkdir()
        fifo = pathlib.Path(state, "release")
        os.mkfifo(fifo)
        socket = "huterm-rgb"
        environment = os.environ.copy()
        environment.update(
            {
                "HOME": str(home),
                "SHELL": "/bin/sh",
                "XDG_CONFIG_HOME": str(pathlib.Path(state, "config")),
                "TERM": "xterm-huterm",
                "TERMINFO": str(terminfo),
                "TERMINFO_DIRS": f"{terminfo}:",
                "TMUX_TMPDIR": state,
            }
        )
        environment.pop("TMUX", None)
        child, master = pty.fork()
        if child == 0:
            command = (
                "printf '\\033[38;2;1;2;3mHUTERM_RGB\\033[0m'; "
                f"IFS= read -r release < {shlex.quote(str(fifo))}"
            )
            os.execvpe(
                "tmux",
                ["tmux", "-L", socket, "-f", "/dev/null", "new-session", "-x", "80", "-y", "24", command],
                environment,
            )

        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        output = bytearray()
        deadline = time.monotonic() + 5
        status = None
        released = False
        try:
            while time.monotonic() < deadline:
                readable, _, _ = select.select([master], [], [], 0.1)
                if readable:
                    try:
                        output.extend(os.read(master, 65536))
                        if len(output) > 1024 * 1024:
                            raise RuntimeError("isolated tmux output exceeded 1 MiB")
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                if WITNESS in output and not released:
                    try:
                        release = os.open(fifo, os.O_WRONLY | os.O_NONBLOCK)
                    except OSError as error:
                        if error.errno != errno.ENXIO:
                            raise
                    else:
                        try:
                            if os.write(release, b"done\n") != len(b"done\n"):
                                raise RuntimeError("short write to isolated tmux release FIFO")
                            released = True
                        finally:
                            os.close(release)
                waited, observed_status = os.waitpid(child, os.WNOHANG)
                if waited == child:
                    status = observed_status
                    break
            if status is None:
                os.kill(child, signal.SIGKILL)
                os.waitpid(child, 0)
                raise RuntimeError("isolated tmux client did not exit")
        finally:
            os.close(master)
            try:
                waited, _ = os.waitpid(child, os.WNOHANG)
                if waited == 0:
                    os.kill(child, signal.SIGKILL)
                    os.waitpid(child, 0)
            except ChildProcessError:
                pass
            subprocess.run(
                ["tmux", "-L", socket, "kill-server"],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=2,
            )

    if not os.waitstatus_to_exitcode(status) == 0:
        raise RuntimeError(f"isolated tmux client exited with status {status}")
    count = output.count(WITNESS)
    if count < 1:
        raise RuntimeError(
            f"expected exact 38;2;1;2;3 SGR and witness text; outer bytes={output.hex()}"
        )
    print(f"TERMINFO_TMUX_RGB exact={WITNESS.hex()} occurrences={count} outer_bytes={output.hex()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
