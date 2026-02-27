# ViewModel fuer die Job-Queue.
# Verwaltet die Liste der Transcode-Jobs und deren Status.
# Kommuniziert mit dem Backend ueber IpcClient und BackendWorker.

from __future__ import annotations

import logging
import os
from typing import Optional

from PyQt6.QtCore import QObject, QThreadPool, pyqtSignal

from proxy_generator.ipc.client import IpcClient
from proxy_generator.models.job import Job, JobMode, JobOptions, JobStatus
from proxy_generator.workers.transcode_worker import BackendWorker

log = logging.getLogger(__name__)


class QueueViewModel(QObject):
    """Holds the job queue and coordinates IPC with the backend."""

    jobs_changed = pyqtSignal()
    job_updated = pyqtSignal(str)  # job_id

    def __init__(self, ipc_client: IpcClient, parent: Optional[QObject] = None) -> None:
        super().__init__(parent)
        self._client = ipc_client
        self._jobs: dict[str, Job] = {}
        self._order: list[str] = []  # insertion order
        self._worker: Optional[BackendWorker] = None

    # -- public API -------------------------------------------------------------

    @property
    def jobs(self) -> list[Job]:
        return [self._jobs[jid] for jid in self._order if jid in self._jobs]

    def set_parallel_jobs(self, count: int) -> None:
        """Send the parallel jobs setting to the backend."""
        try:
            self._client.set_parallel_jobs(count)
        except (RuntimeError, BrokenPipeError, OSError) as e:
            log.error("Failed to set parallel jobs: %s", e)

    def add_files(
        self,
        paths: list[str],
        output_dir: str,
        mode: JobMode,
        options: JobOptions,
    ) -> None:
        """Create Job objects for each path and send them to the backend."""
        existing_paths = {job.input_path for job in self._jobs.values()}

        for p in paths:
            if p in existing_paths:
                log.warning("Datei bereits in Queue: %s", p)
                continue
            if not os.path.isfile(p):
                log.warning("Datei nicht gefunden: %s", p)
                continue
            if not os.access(p, os.R_OK):
                log.warning("Keine Leseberechtigung: %s", p)
                continue
            job = Job(
                input_path=p,
                output_dir=output_dir,
                mode=mode,
                options=options,
            )
            try:
                self._client.add_job(job)
                self._jobs[job.id] = job
                self._order.append(job.id)
            except (RuntimeError, BrokenPipeError, OSError) as e:
                log.error("Failed to add job %s: %s", p, e)
        self.jobs_changed.emit()

    def start_worker(self) -> None:
        """Start the backend reader worker in the thread pool."""
        if self._worker is not None:
            return
        self._worker = BackendWorker(self._client)
        self._worker.signals.job_progress.connect(self._on_progress)
        self._worker.signals.job_done.connect(self._on_done)
        self._worker.signals.job_error.connect(self._on_error)
        self._worker.signals.job_queued.connect(self._on_queued)
        self._worker.signals.job_cancelled.connect(self._on_cancelled)
        self._worker.signals.connection_lost.connect(self._on_connection_lost)
        QThreadPool.globalInstance().start(self._worker)

    def cancel_job(self, job_id: str) -> None:
        job = self._jobs.get(job_id)
        if job is None:
            return
        if job.status in (JobStatus.QUEUED, JobStatus.RUNNING):
            try:
                self._client.cancel_job(job_id)
            except (RuntimeError, BrokenPipeError, OSError) as e:
                log.error("Failed to cancel job %s: %s", job_id, e)

    def remove_job(self, job_id: str) -> None:
        """Remove a single job from the queue."""
        if job_id in self._jobs:
            job = self._jobs[job_id]
            if job.status in (JobStatus.QUEUED, JobStatus.RUNNING):
                try:
                    self._client.cancel_job(job_id)
                except (RuntimeError, BrokenPipeError, OSError) as e:
                    log.error("Failed to cancel job %s during remove: %s", job_id, e)
            del self._jobs[job_id]
            self._order = [jid for jid in self._order if jid != job_id]
            self.jobs_changed.emit()

    def clear_all(self) -> None:
        """Alle Jobs aus der Queue entfernen (auch laufende werden abgebrochen)."""
        for job_id in list(self._jobs.keys()):
            if self._jobs[job_id].status in (JobStatus.RUNNING, JobStatus.QUEUED):
                try:
                    self._client.cancel_job(job_id)
                except Exception:
                    pass
        self._jobs.clear()
        self._order.clear()
        self.jobs_changed.emit()

    def clear_done(self) -> None:
        """Remove all completed / errored / cancelled jobs."""
        to_remove = [
            jid for jid, job in self._jobs.items()
            if job.status in (JobStatus.DONE, JobStatus.ERROR, JobStatus.CANCELLED)
        ]
        for jid in to_remove:
            del self._jobs[jid]
        self._order = [jid for jid in self._order if jid in self._jobs]
        if to_remove:
            self.jobs_changed.emit()

    def stop(self) -> None:
        """Stop the worker and the backend."""
        if self._worker is not None:
            self._worker.stop()
            QThreadPool.globalInstance().waitForDone(3000)
            self._worker = None
        self._client.stop()

    # -- slots from worker signals ----------------------------------------------

    def _on_queued(self, job_id: str) -> None:
        job = self._jobs.get(job_id)
        if job:
            job.status = JobStatus.QUEUED
            self.job_updated.emit(job_id)

    def _on_progress(self, job_id: str, percent: float, fps: float, speed: float) -> None:
        job = self._jobs.get(job_id)
        if job is None:
            return
        job.status = JobStatus.RUNNING
        job.progress = percent
        job.fps = fps
        job.speed = speed
        self.job_updated.emit(job_id)

    def _on_done(self, job_id: str) -> None:
        job = self._jobs.get(job_id)
        if job is None:
            return
        job.status = JobStatus.DONE
        job.progress = 100.0
        self.job_updated.emit(job_id)

    def _on_cancelled(self, job_id: str) -> None:
        job = self._jobs.get(job_id)
        if job:
            job.status = JobStatus.CANCELLED
            self.job_updated.emit(job_id)

    def _on_error(self, job_id: str, message: str) -> None:
        job = self._jobs.get(job_id)
        if job is None:
            return
        job.status = JobStatus.ERROR
        job.error = message
        self.job_updated.emit(job_id)

    def _on_connection_lost(self) -> None:
        # Mark all running/queued jobs as error
        for job in self._jobs.values():
            if job.status in (JobStatus.QUEUED, JobStatus.RUNNING):
                job.status = JobStatus.ERROR
                job.error = "Verbindung zum Backend verloren"
        if self._worker is not None:
            try:
                self._worker.signals.job_progress.disconnect()
                self._worker.signals.job_done.disconnect()
                self._worker.signals.job_error.disconnect()
                self._worker.signals.job_queued.disconnect()
                self._worker.signals.job_cancelled.disconnect()
                self._worker.signals.connection_lost.disconnect()
            except RuntimeError:
                pass  # already disconnected
            self._worker = None
        self._client.stop()
        self.jobs_changed.emit()
