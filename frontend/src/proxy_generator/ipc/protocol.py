# IPC-Protokoll: Nachrichten-Typen fuer die Kommunikation mit dem Rust-Backend.
# Muss mit backend/src/ipc/protocol.rs konsistent bleiben.
# Kommunikation erfolgt ueber NDJSON auf stdin/stdout.

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


# ---------------------------------------------------------------------------
# Requests (Python -> Rust)
# ---------------------------------------------------------------------------

@dataclass
class AddJobRequest:
    id: str
    input_path: str
    output_dir: str
    mode: str
    options: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        d = {
            "type": "add_job",
            "id": self.id,
            "input_path": self.input_path,
            "output_dir": self.output_dir,
            "mode": self.mode,
        }
        if self.options:
            d["options"] = self.options
        return d


@dataclass
class CancelJobRequest:
    id: str

    def to_dict(self) -> dict:
        return {"type": "cancel_job", "id": self.id}


@dataclass
class GetStatusRequest:
    def to_dict(self) -> dict:
        return {"type": "get_status"}


@dataclass
class SetParallelJobsRequest:
    count: int

    def to_dict(self) -> dict:
        return {"type": "set_parallel_jobs", "count": self.count}


@dataclass
class ShutdownRequest:
    def to_dict(self) -> dict:
        return {"type": "shutdown"}


# ---------------------------------------------------------------------------
# Responses (Rust -> Python)
# ---------------------------------------------------------------------------

@dataclass
class JobQueuedResponse:
    id: str

    @classmethod
    def from_dict(cls, data: dict) -> JobQueuedResponse:
        return cls(id=data["id"])


@dataclass
class JobProgressResponse:
    id: str
    percent: float
    fps: float
    speed: float
    frame: int

    @classmethod
    def from_dict(cls, data: dict) -> JobProgressResponse:
        return cls(
            id=data["id"],
            percent=data.get("percent", 0.0),
            fps=data.get("fps", 0.0),
            speed=data.get("speed", 0.0),
            frame=data.get("frame", 0),
        )


@dataclass
class JobDoneResponse:
    id: str

    @classmethod
    def from_dict(cls, data: dict) -> JobDoneResponse:
        return cls(id=data["id"])


@dataclass
class JobErrorResponse:
    id: str
    message: str

    @classmethod
    def from_dict(cls, data: dict) -> JobErrorResponse:
        return cls(id=data["id"], message=data.get("message", "Unknown error"))


@dataclass
class JobStatusEntry:
    id: str
    input_path: str
    mode: str
    status: str
    percent: float


@dataclass
class StatusReportResponse:
    jobs: list[JobStatusEntry]

    @classmethod
    def from_dict(cls, data: dict) -> StatusReportResponse:
        jobs = []
        for j in data.get("jobs", []):
            jobs.append(JobStatusEntry(
                id=j["id"],
                input_path=j["input_path"],
                mode=j["mode"],
                status=j["status"],
                percent=j.get("percent", 0.0),
            ))
        return cls(jobs=jobs)


# ---------------------------------------------------------------------------
# Dispatcher: raw dict -> typed response
# ---------------------------------------------------------------------------

_RESPONSE_MAP = {
    "job_queued": JobQueuedResponse,
    "job_progress": JobProgressResponse,
    "job_done": JobDoneResponse,
    "job_error": JobErrorResponse,
    "status_report": StatusReportResponse,
}


def parse_response(data: dict) -> Optional[object]:
    """Parse a raw JSON dict into a typed response dataclass."""
    msg_type = data.get("type")
    cls = _RESPONSE_MAP.get(msg_type)
    if cls is None:
        return None
    return cls.from_dict(data)
