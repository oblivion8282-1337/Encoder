# Worker: Liest kontinuierlich Antworten vom Rust-Backend und leitet sie
# als Qt-Signals an das ViewModel weiter.  Laeuft in QThreadPool.

from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from PyQt6.QtCore import QObject, QRunnable, pyqtSignal, pyqtSlot

from proxy_generator.ipc.protocol import (
    JobCancelledResponse,
    JobDoneResponse,
    JobErrorResponse,
    JobProgressResponse,
    JobQueuedResponse,
    StatusReportResponse,
)

if TYPE_CHECKING:
    from proxy_generator.ipc.client import IpcClient

log = logging.getLogger(__name__)


class WorkerSignals(QObject):
    """Signals emitted by BackendWorker (must be on QObject, not QRunnable)."""

    job_progress = pyqtSignal(str, float, float, float)  # id, percent, fps, speed
    job_done = pyqtSignal(str)                            # id
    job_error = pyqtSignal(str, str)                      # id, message
    job_queued = pyqtSignal(str)                          # id
    job_cancelled = pyqtSignal(str)                       # id
    connection_lost = pyqtSignal()


class BackendWorker(QRunnable):
    """Reads responses from the IPC client in a background thread."""

    def __init__(self, ipc_client: IpcClient) -> None:
        super().__init__()
        self.signals = WorkerSignals()
        self._client = ipc_client
        self._running = True
        self.setAutoDelete(False)

    def stop(self) -> None:
        self._running = False

    @pyqtSlot()
    def run(self) -> None:
        """Main loop: read responses and dispatch to signals."""
        try:
            for response in self._client.read_responses():
                if not self._running:
                    break
                self._dispatch(response)
        except Exception:
            log.exception("Backend reader error")
        finally:
            if self._running:
                self.signals.connection_lost.emit()
            log.info("Backend worker stopped")

    def _dispatch(self, response: object) -> None:
        if isinstance(response, JobProgressResponse):
            self.signals.job_progress.emit(
                response.id, response.percent, response.fps, response.speed,
            )
        elif isinstance(response, JobDoneResponse):
            self.signals.job_done.emit(response.id)
        elif isinstance(response, JobErrorResponse):
            self.signals.job_error.emit(response.id, response.message)
        elif isinstance(response, JobQueuedResponse):
            self.signals.job_queued.emit(response.id)
        elif isinstance(response, JobCancelledResponse):
            self.signals.job_cancelled.emit(response.id)
        elif isinstance(response, StatusReportResponse):
            pass  # could be handled if needed
        else:
            log.warning("Unhandled response: %r", response)
