# IPC-Client: Startet das Rust-Backend als Subprocess und kommuniziert
# ueber NDJSON auf stdin/stdout.

from __future__ import annotations

import json
import logging
import os
import subprocess
from pathlib import Path
from typing import Generator, Optional

from proxy_generator.ipc.protocol import (
    AddJobRequest,
    CancelJobRequest,
    GetStatusRequest,
    SetMaxParallelRequest,
    ShutdownRequest,
    parse_response,
)
from proxy_generator.models.job import Job

log = logging.getLogger(__name__)


def _find_backend_binary() -> str:
    """Locate the backend binary.

    Resolution order:
      1. PROXY_GENERATOR_BACKEND environment variable
      2. Sibling to this package's directory (../../../proxy-generator-backend)
      3. On PATH as 'proxy-generator-backend'
    """
    env = os.environ.get("PROXY_GENERATOR_BACKEND")
    if env:
        return env

    # Relative to this file: frontend/src/proxy_generator/ipc/client.py
    # -> up 4 levels to project root, then backend binary
    project_root = Path(__file__).resolve().parents[4]
    candidate = project_root / "backend" / "target" / "release" / "proxy-generator-backend"
    if candidate.is_file():
        return str(candidate)
    candidate_debug = project_root / "backend" / "target" / "debug" / "proxy-generator-backend"
    if candidate_debug.is_file():
        return str(candidate_debug)

    # Fallback: assume on PATH
    return "proxy-generator-backend"


def find_backend_binary() -> str:
    """Public wrapper: Locate the backend binary. See _find_backend_binary for details."""
    return _find_backend_binary()


class IpcClient:
    """Manages the Rust backend subprocess and provides IPC over NDJSON."""

    def __init__(self, backend_path: Optional[str] = None) -> None:
        self._backend_path = backend_path or _find_backend_binary()
        self._process: Optional[subprocess.Popen] = None

    @property
    def is_running(self) -> bool:
        return self._process is not None and self._process.poll() is None

    def start(self, max_parallel: int = 1) -> None:
        """Start the backend subprocess."""
        if self.is_running:
            return
        log.info("Starting backend: %s (max_parallel=%d)", self._backend_path, max_parallel)
        self._process = subprocess.Popen(
            [self._backend_path, "--max-parallel", str(max_parallel)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,  # line-buffered
        )

    def stop(self) -> None:
        """Send shutdown command and terminate the backend."""
        if not self.is_running:
            return
        try:
            self._send_message(ShutdownRequest().to_dict())
        except (BrokenPipeError, OSError):
            pass
        try:
            self._process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._process.terminate()
            try:
                self._process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self._process.kill()
        self._process = None

    def add_job(self, job: Job) -> None:
        """Send an add_job request for the given Job."""
        options = {
            "audio_codec": job.options.audio_codec,
            "proxy_codec": job.options.proxy_codec,
            "hw_accel": job.options.hw_accel,
            "output_suffix": job.options.output_suffix,
            "output_subfolder": job.options.output_subfolder,
            "skip_if_exists": job.options.skip_if_exists,
        }
        if job.options.proxy_resolution is not None:
            options["proxy_resolution"] = job.options.proxy_resolution

        req = AddJobRequest(
            id=job.id,
            input_path=job.input_path,
            output_dir=job.output_dir,
            mode=job.mode.value,
            options=options,
        )
        self._send_message(req.to_dict())

    def set_max_parallel(self, n: int) -> None:
        """Send a set_max_parallel request."""
        self._send_message(SetMaxParallelRequest(n=max(1, n)).to_dict())

    def cancel_job(self, job_id: str) -> None:
        """Send a cancel_job request."""
        self._send_message(CancelJobRequest(id=job_id).to_dict())

    def get_status(self) -> None:
        """Send a get_status request."""
        self._send_message(GetStatusRequest().to_dict())

    def read_responses(self) -> Generator:
        """Blocking generator: yields parsed response objects from stdout.

        This should be called from a worker thread, not the main thread.
        """
        if self._process is None or self._process.stdout is None:
            return
        for line in self._process.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                log.warning("Invalid JSON from backend: %s", line[:200])
                continue
            response = parse_response(data)
            if response is not None:
                yield response
            else:
                log.warning("Unknown response type: %s", data.get("type"))

    # -- internal ---------------------------------------------------------------

    def _send_message(self, msg: dict) -> None:
        if self._process is None or self._process.stdin is None:
            raise RuntimeError("Backend not running")
        line = json.dumps(msg, separators=(",", ":")) + "\n"
        self._process.stdin.write(line)
        self._process.stdin.flush()
