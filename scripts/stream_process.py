"""Bounded process output streaming with callbacks and a total deadline."""
from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import selectors
import signal
import subprocess
import time
from typing import Callable


class StreamError(RuntimeError):
    def __init__(self, message: str, stdout: str):
        super().__init__(message)
        self.stdout = stdout


@dataclass
class StreamResult:
    returncode: int
    stdout: str


def stream_process(argv: list[str], *, cwd: Path, env: dict, timeout: float,
                   output_path: Path, on_line: Callable[[str], None],
                   output_limit: int = 64 * 1024 * 1024, line_limit: int = 1024 * 1024) -> StreamResult:
    started = time.monotonic()
    raw = bytearray()
    pending = bytearray()
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0, start_new_session=True)
    assert process.stdout is not None
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    os.set_blocking(process.stdout.fileno(), False)
    try:
        with output_path.open('wb') as log:
            while selector.get_map():
                remaining = timeout - (time.monotonic() - started)
                if remaining <= 0:
                    raise TimeoutError(f'{Path(argv[0]).name} timed out after {timeout:g}s.')
                for key, _events in selector.select(min(remaining, 0.5)):
                    chunk = os.read(key.fileobj.fileno(), 64 * 1024)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        if pending:
                            on_line(pending.decode('utf-8', errors='replace'))
                            pending.clear()
                        continue
                    raw.extend(chunk)
                    log.write(chunk)
                    log.flush()
                    if len(raw) > output_limit:
                        raise ValueError('Instrument output exceeds the bounded log size.')
                    pending.extend(chunk)
                    while b'\n' in pending:
                        newline = pending.index(b'\n')
                        if newline > line_limit:
                            raise ValueError('Instrument output line exceeds the bounded line size.')
                        line = pending[:newline].decode('utf-8', errors='replace')
                        del pending[:newline + 1]
                        on_line(line)
                        if time.monotonic() - started >= timeout:
                            raise TimeoutError(f'{Path(argv[0]).name} timed out after {timeout:g}s.')
                    if len(pending) > line_limit:
                        raise ValueError('Instrument output line exceeds the bounded line size.')
            remaining = timeout - (time.monotonic() - started)
            if remaining <= 0:
                raise TimeoutError(f'{Path(argv[0]).name} timed out after {timeout:g}s.')
            return StreamResult(process.wait(timeout=remaining), raw.decode('utf-8', errors='replace'))
    except Exception as error:
        raise StreamError(str(error), raw.decode('utf-8', errors='replace')) from None
    finally:
        selector.close()
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=1)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=2)
        process.stdout.close()
