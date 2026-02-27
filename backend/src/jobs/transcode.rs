// Job-Queue: Verwaltet Transcode-Jobs mit konfigurierbarer Parallelitaet.
// Nutzt tokio::sync::mpsc fuer Job-Eingang und Semaphore fuer Parallelitaet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::runner::{self, build_ffmpeg_args, FfmpegEvent};
use crate::ipc::protocol::{JobMode, JobOptions, JobState, JobStatus, Response};

/// Validiert einen Pfad gegen Path-Traversal-Angriffe.
/// Stellt sicher, dass der kanonische Pfad nicht ausserhalb erlaubter Bereiche liegt.
fn validate_path(p: &Path) -> Result<PathBuf> {
    let path = p
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Ungueltiger Pfad {:?}: {e}", p))?;
    // Verweigere offensichtliche Traversal-Versuche
    let path_str = path.to_string_lossy();
    if path_str.contains("..") {
        return Err(anyhow::anyhow!("Path-Traversal erkannt in: {:?}", p));
    }
    Ok(path)
}

/// Ein einzelner Transcode-Auftrag.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub input_path: PathBuf,
    pub output_dir: PathBuf,
    pub mode: JobMode,
    pub options: JobOptions,
    pub status: JobState,
    pub percent: f32,
    pub cancel_token: CancellationToken,
}

impl Job {
    pub fn new(
        id: String,
        input_path: String,
        output_dir: String,
        mode: JobMode,
        options: JobOptions,
    ) -> Self {
        Self {
            id,
            input_path: PathBuf::from(input_path),
            output_dir: PathBuf::from(output_dir),
            mode,
            options,
            status: JobState::Queued,
            percent: 0.0,
            cancel_token: CancellationToken::new(), // Wird spaeter durch child_token ersetzt
        }
    }

    /// Ersetzt das cancel_token durch ein Child-Token des gegebenen Parent-Tokens.
    /// So wird der Job abgebrochen wenn entweder explizit gecancelt oder global shutdown.
    pub fn attach_to_parent_token(&mut self, parent: &CancellationToken) {
        self.cancel_token = parent.child_token();
    }

    /// Generiert den Output-Pfad basierend auf Modus.
    pub fn output_path(&self) -> PathBuf {
        let stem = self
            .input_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();

        let (suffix, ext) = match self.mode {
            JobMode::Proxy => ("_proxy", "mp4"),
            JobMode::ReWrap => ("_rewrap", "mov"),
        };

        self.output_dir.join(format!("{stem}{suffix}.{ext}"))
    }

    pub fn to_status(&self) -> JobStatus {
        JobStatus {
            id: self.id.clone(),
            input_path: self.input_path.to_string_lossy().to_string(),
            mode: self.mode.clone(),
            status: self.status.clone(),
            percent: self.percent,
        }
    }
}

/// Kommandos die an die JobQueue geschickt werden koennen.
pub enum JobCommand {
    Add(Job),
    Cancel(String),
    GetStatus(tokio::sync::oneshot::Sender<Vec<JobStatus>>),
}

/// Die zentrale Job-Queue.
pub struct JobQueue {
    cmd_tx: mpsc::Sender<JobCommand>,
    shutdown_token: CancellationToken,
}

impl JobQueue {
    /// Erstellt eine neue JobQueue und gibt (queue, event_receiver) zurueck.
    /// `max_parallel` bestimmt wie viele Jobs gleichzeitig laufen duerfen.
    pub fn new(
        _max_parallel: usize,
        _response_tx: mpsc::Sender<Response>,
    ) -> (Self, mpsc::Receiver<JobCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        let shutdown_token = CancellationToken::new();
        (Self { cmd_tx, shutdown_token }, cmd_rx)
    }

    /// Gibt das Shutdown-Token zurueck, um es beim Herunterfahren zu cancellen.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    pub async fn add_job(&self, job: Job) -> Result<()> {
        self.cmd_tx.send(JobCommand::Add(job)).await?;
        Ok(())
    }

    pub async fn cancel_job(&self, id: String) -> Result<()> {
        self.cmd_tx.send(JobCommand::Cancel(id)).await?;
        Ok(())
    }

    pub async fn get_status(&self) -> Result<Vec<JobStatus>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx.send(JobCommand::GetStatus(tx)).await?;
        Ok(rx.await?)
    }
}

/// Laeuft als eigener Task und verarbeitet Job-Kommandos.
pub async fn run_queue(
    mut cmd_rx: mpsc::Receiver<JobCommand>,
    max_parallel: usize,
    response_tx: mpsc::Sender<Response>,
    shutdown_token: CancellationToken,
) {
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let jobs: Arc<Mutex<HashMap<String, Job>>> = Arc::new(Mutex::new(HashMap::new()));

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            JobCommand::Add(mut job) => {
                let job_id = job.id.clone();

                // Pfade validieren (Path-Traversal-Schutz)
                let input_path = match validate_path(&job.input_path) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = response_tx
                            .send(Response::JobError {
                                id: job_id,
                                message: format!("Ungueltiger Input-Pfad: {e}"),
                            })
                            .await;
                        continue;
                    }
                };
                job.input_path = input_path;

                // Output-Verzeichnis anlegen falls noetig
                if let Err(e) = tokio::fs::create_dir_all(&job.output_dir).await {
                    let _ = response_tx
                        .send(Response::JobError {
                            id: job_id,
                            message: format!("Output-Verzeichnis kann nicht erstellt werden: {e}"),
                        })
                        .await;
                    continue;
                }

                // Output-Verzeichnis nach Erstellung validieren
                let output_dir = match validate_path(&job.output_dir) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = response_tx
                            .send(Response::JobError {
                                id: job_id,
                                message: format!("Ungueltiger Output-Pfad: {e}"),
                            })
                            .await;
                        continue;
                    }
                };
                job.output_dir = output_dir;

                // Job an globales Shutdown-Token haengen
                job.attach_to_parent_token(&shutdown_token);
                let cancel_token = job.cancel_token.clone();
                let output_path = job.output_path();
                let args = build_ffmpeg_args(
                    &job.input_path,
                    &output_path,
                    &job.mode,
                    &job.options,
                );

                // Dauer der Quelldatei ermitteln (via ffprobe)
                let input_path_clone = job.input_path.clone();
                let total_duration_us = match probe_duration(&input_path_clone).await {
                    Ok(d) if d > 0 => d,
                    Ok(_) => {
                        eprintln!("Warnung: ffprobe lieferte Dauer 0 fuer {:?}", input_path_clone);
                        0
                    }
                    Err(e) => {
                        eprintln!("Warnung: ffprobe fehlgeschlagen fuer {:?}: {e}", input_path_clone);
                        0
                    }
                };

                job.status = JobState::Queued;
                {
                    let mut map = jobs.lock().await;
                    map.insert(job_id.clone(), job);
                }

                // JobQueued senden
                let _ = response_tx
                    .send(Response::JobQueued { id: job_id.clone() })
                    .await;

                let sem = semaphore.clone();
                let resp_tx = response_tx.clone();
                let jobs_ref = jobs.clone();

                tokio::spawn(async move {
                    // Warten bis ein Slot frei ist
                    let _permit = match sem.acquire().await {
                        Ok(p) => p,
                        Err(_) => {
                            eprintln!("Semaphore geschlossen, Job {job_id} kann nicht starten");
                            return;
                        }
                    };

                    // Status auf Running setzen
                    {
                        let mut map = jobs_ref.lock().await;
                        if let Some(j) = map.get_mut(&job_id) {
                            j.status = JobState::Running;
                        }
                    }

                    // Event-Channel fuer diesen FFmpeg-Lauf
                    let (event_tx, mut event_rx) = mpsc::channel::<FfmpegEvent>(64);

                    // FFmpeg in eigenem Task starten
                    let ffmpeg_id = job_id.clone();
                    let ffmpeg_handle = tokio::spawn(async move {
                        runner::run_ffmpeg(
                            ffmpeg_id,
                            args,
                            total_duration_us,
                            event_tx,
                            cancel_token,
                        )
                        .await
                    });

                    // Events weiterleiten an IPC
                    while let Some(event) = event_rx.recv().await {
                        match event {
                            FfmpegEvent::Progress {
                                id,
                                percent,
                                fps,
                                speed,
                                frame,
                            } => {
                                {
                                    let mut map = jobs_ref.lock().await;
                                    if let Some(j) = map.get_mut(&id) {
                                        j.percent = percent;
                                    }
                                }
                                let _ = resp_tx
                                    .send(Response::JobProgress {
                                        id,
                                        percent,
                                        fps,
                                        speed,
                                        frame,
                                    })
                                    .await;
                            }
                            FfmpegEvent::Done { id } => {
                                {
                                    let mut map = jobs_ref.lock().await;
                                    if let Some(j) = map.get_mut(&id) {
                                        j.status = JobState::Done;
                                        j.percent = 100.0;
                                    }
                                }
                                let _ = resp_tx.send(Response::JobDone { id }).await;
                            }
                            FfmpegEvent::Error { id, message } => {
                                {
                                    let mut map = jobs_ref.lock().await;
                                    if let Some(j) = map.get_mut(&id) {
                                        j.status = JobState::Error;
                                    }
                                }
                                let _ = resp_tx
                                    .send(Response::JobError { id, message })
                                    .await;
                            }
                            FfmpegEvent::Cancelled { id } => {
                                {
                                    let mut map = jobs_ref.lock().await;
                                    if let Some(j) = map.get_mut(&id) {
                                        j.status = JobState::Cancelled;
                                    }
                                }
                                let _ = resp_tx
                                    .send(Response::JobCancelled { id })
                                    .await;
                            }
                        }
                    }

                    let _ = ffmpeg_handle.await;
                });
            }
            JobCommand::Cancel(id) => {
                let map = jobs.lock().await;
                if let Some(job) = map.get(&id) {
                    job.cancel_token.cancel();
                }
            }
            JobCommand::GetStatus(reply) => {
                let map = jobs.lock().await;
                let statuses: Vec<JobStatus> = map.values().map(|j| j.to_status()).collect();
                let _ = reply.send(statuses);
            }
        }
    }
}

/// Ermittelt die Dauer einer Mediendatei in Mikrosekunden via ffprobe.
async fn probe_duration(path: &Path) -> Result<i64> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.as_os_str())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("ffprobe konnte nicht gestartet werden: {e}"))?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "ffprobe beendet mit Exit-Code: {}",
            output.status.code().unwrap_or(-1)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let seconds: f64 = stdout
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("ffprobe Dauer nicht parsebar '{}': {e}", stdout.trim()))?;
    Ok((seconds * 1_000_000.0) as i64)
}
