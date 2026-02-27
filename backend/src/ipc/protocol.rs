// IPC-Protokoll: Serde-Typen fuer alle Requests und Responses.
// NDJSON ueber stdin/stdout (JSON-RPC-aehnlich).
// Muss mit frontend/src/proxy_generator/ipc/protocol.py konsistent bleiben.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Eingehend (von Python)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    #[serde(rename = "add_job")]
    AddJob {
        id: String,
        input_path: String,
        output_dir: String,
        mode: JobMode,
        #[serde(default)]
        options: JobOptions,
    },

    #[serde(rename = "cancel_job")]
    CancelJob { id: String },

    #[serde(rename = "get_status")]
    GetStatus,

    #[serde(rename = "shutdown")]
    Shutdown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobMode {
    ReWrap,
    Proxy,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JobOptions {
    #[serde(default = "default_audio_codec")]
    pub audio_codec: String,

    pub proxy_resolution: Option<String>,

    // Werte: "h264" | "h265" | "av1" | "prores_proxy" | "prores_lt" | "prores_422" | "prores_hq"
    #[serde(default = "default_proxy_codec")]
    pub proxy_codec: String,

    #[serde(default = "default_hw_accel")]
    pub hw_accel: String,

    /// Suffix vor der Dateiendung, z.B. "_proxy". Leer = kein Suffix.
    #[serde(default = "default_output_suffix")]
    pub output_suffix: String,

    /// Optionaler Unterordner im Ausgabeverzeichnis, z.B. "proxy". Leer = keiner.
    #[serde(default)]
    pub output_subfolder: String,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            audio_codec: default_audio_codec(),
            proxy_resolution: None,
            proxy_codec: default_proxy_codec(),
            hw_accel: default_hw_accel(),
            output_suffix: default_output_suffix(),
            output_subfolder: String::new(),
        }
    }
}

fn default_audio_codec() -> String {
    "pcm_s24le".to_string()
}

fn default_proxy_codec() -> String {
    "h264".to_string()
}

fn default_hw_accel() -> String {
    "none".to_string()
}

fn default_output_suffix() -> String {
    String::new()
}

// ---------------------------------------------------------------------------
// Ausgehend (zu Python)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Response {
    #[serde(rename = "job_queued")]
    JobQueued { id: String },

    #[serde(rename = "job_progress")]
    JobProgress {
        id: String,
        percent: f32,
        fps: f32,
        speed: f32,
        frame: u64,
    },

    #[serde(rename = "job_done")]
    JobDone { id: String },

    #[serde(rename = "job_error")]
    JobError { id: String, message: String },

    #[serde(rename = "job_cancelled")]
    JobCancelled { id: String },

    #[serde(rename = "status_report")]
    StatusReport { jobs: Vec<JobStatus> },
}

#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    pub id: String,
    pub input_path: String,
    pub mode: JobMode,
    pub status: JobState,
    pub percent: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}
