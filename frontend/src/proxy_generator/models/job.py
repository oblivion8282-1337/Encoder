# Datenmodell fuer einen einzelnen Transcode-Job.

from dataclasses import dataclass, field
from enum import Enum
from typing import Optional
import uuid


class JobMode(Enum):
    REWRAP = "re_wrap"
    PROXY = "proxy"


class JobStatus(Enum):
    QUEUED = "queued"
    RUNNING = "running"
    DONE = "done"
    ERROR = "error"
    CANCELLED = "cancelled"


@dataclass
class JobOptions:
    audio_codec: str = "pcm_s24le"
    proxy_resolution: Optional[str] = None
    proxy_codec: str = "h264"
    hw_accel: str = "none"
    output_suffix: str = ""
    output_subfolder: str = ""
    skip_if_exists: bool = False


@dataclass
class Job:
    input_path: str
    output_dir: str
    mode: JobMode
    options: JobOptions = field(default_factory=JobOptions)
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    status: JobStatus = JobStatus.QUEUED
    progress: float = 0.0
    fps: float = 0.0
    speed: float = 0.0
    error: Optional[str] = None
