// Job-Queue: Verwaltet Transcode-Jobs mit konfigurierbarer Parallelitaet.
// Nutzt tokio::sync::mpsc fuer Job-Eingang und Semaphore fuer Parallelitaet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::runner::{self, build_ffmpeg_args, FfmpegEvent};
#[allow(unused_imports)]
use libc;
use crate::ipc::protocol::{JobMode, JobOptions, JobState, JobStatus, Response};

/// Validiert einen Pfad gegen Path-Traversal-Angriffe.
/// Stellt sicher, dass der kanonische Pfad nicht ausserhalb erlaubter Bereiche liegt.
fn validate_path(p: &Path) -> Result<PathBuf> {
    p.canonicalize()
        .map_err(|e| anyhow::anyhow!("Ungueltiger Pfad {:?}: {e}", p))
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

    /// Generiert den Output-Pfad basierend auf Modus und Benennungsoptionen.
    pub fn output_path(&self) -> PathBuf {
        let stem = self
            .input_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();

        let suffix = &self.options.output_suffix;

        let ext = match self.mode {
            JobMode::ReWrap => "mov",
            JobMode::Proxy  => if self.options.proxy_codec == "av1" { "mp4" } else { "mov" },
        };

        let output_dir = if self.options.output_subfolder.is_empty() {
            self.output_dir.clone()
        } else {
            self.output_dir.join(&self.options.output_subfolder)
        };

        output_dir.join(format!("{stem}{suffix}.{ext}"))
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
    SetMaxParallel(usize),
    PauseAll,
    ResumeAll,
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

    pub async fn set_max_parallel(&self, n: usize) -> Result<()> {
        self.cmd_tx.send(JobCommand::SetMaxParallel(n.max(1))).await?;
        Ok(())
    }

    pub async fn pause_all(&self) -> Result<()> {
        self.cmd_tx.send(JobCommand::PauseAll).await?;
        Ok(())
    }

    pub async fn resume_all(&self) -> Result<()> {
        self.cmd_tx.send(JobCommand::ResumeAll).await?;
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
    let limit = Arc::new(AtomicUsize::new(max_parallel.max(1)));
    let running = Arc::new(AtomicUsize::new(0));
    let slot_free = Arc::new(Notify::new());
    let is_paused = Arc::new(AtomicBool::new(false));
    // job_id → PID des laufenden FFmpeg-Prozesses (0 = noch nicht gestartet)
    let ffmpeg_pids: Arc<RwLock<HashMap<String, Arc<AtomicU32>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let jobs: Arc<RwLock<HashMap<String, Job>>> = Arc::new(RwLock::new(HashMap::new()));

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

                // Output-Verzeichnis anlegen (inkl. optionalem Unterordner)
                let output_base = if job.options.output_subfolder.is_empty() {
                    job.output_dir.clone()
                } else {
                    job.output_dir.join(&job.options.output_subfolder)
                };
                if let Err(e) = tokio::fs::create_dir_all(&output_base).await {
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

                // Dauer und Pixel-Format der Quelldatei ermitteln (parallel via ffprobe).
                // Das Pixel-Format wird benoetigt um fuer NVENC die optimale Pipeline
                // zu waehlen (volle GPU vs. Hybrid mit CPU-Format-Konvertierung).
                let input_path_clone = job.input_path.clone();
                let needs_pix_fmt = matches!(job.mode, JobMode::Proxy)
                    && job.options.hw_accel == "nvenc";
                let (duration_result, pix_fmt) = tokio::join!(
                    probe_duration(&input_path_clone),
                    async {
                        if needs_pix_fmt {
                            probe_pix_fmt(&input_path_clone).await
                        } else {
                            String::new()
                        }
                    },
                );
                let total_duration_us = match duration_result {
                    Ok(d) if d > 0 => d,
                    Ok(_) | Err(_) => {
                        let _ = response_tx
                            .send(Response::JobError {
                                id: job_id.clone(),
                                message: "Quelldatei konnte nicht gelesen werden (ffprobe fehlgeschlagen)".to_string(),
                            })
                            .await;
                        continue;
                    }
                };
                let nvenc_full_gpu = nvenc_full_gpu_supported(&pix_fmt);

                // Ausgabedatei bereits vorhanden und Skip aktiviert?
                let output_path = job.output_path();
                if job.options.skip_if_exists && output_path.exists() {
                    let _ = response_tx
                        .send(Response::JobQueued { id: job_id.clone() })
                        .await;
                    let _ = response_tx
                        .send(Response::JobDone { id: job_id })
                        .await;
                    continue;
                }

                // Job an globales Shutdown-Token haengen
                job.attach_to_parent_token(&shutdown_token);
                let cancel_token = job.cancel_token.clone();
                let args = build_ffmpeg_args(
                    &job.input_path,
                    &output_path,
                    &job.mode,
                    &job.options,
                    nvenc_full_gpu,
                );

                job.status = JobState::Queued;
                {
                    let mut map = jobs.write().await;
                    map.insert(job_id.clone(), job);
                }

                // JobQueued senden
                let _ = response_tx
                    .send(Response::JobQueued { id: job_id.clone() })
                    .await;

                let limit_ref = limit.clone();
                let running_ref = running.clone();
                let slot_free_ref = slot_free.clone();
                let is_paused_ref = is_paused.clone();
                let pid_slot = Arc::new(AtomicU32::new(0));
                {
                    ffmpeg_pids.write().await.insert(job_id.clone(), pid_slot.clone());
                }
                let ffmpeg_pids_ref = ffmpeg_pids.clone();
                let resp_tx = response_tx.clone();
                let jobs_ref = jobs.clone();
                let job_id_for_monitor = job_id.clone();

                let handle = tokio::spawn(async move {
                    // Warten bis ein Slot frei ist UND nicht pausiert – oder Job wird gecancelt
                    loop {
                        if is_paused_ref.load(Ordering::Acquire) {
                            tokio::select! {
                                _ = slot_free_ref.notified() => continue,
                                _ = cancel_token.cancelled() => {
                                    ffmpeg_pids_ref.write().await.remove(&job_id);
                                    jobs_ref.write().await.remove(&job_id);
                                    let _ = resp_tx.send(Response::JobCancelled { id: job_id.clone() }).await;
                                    return;
                                }
                            }
                        }
                        let cur = running_ref.load(Ordering::Acquire);
                        let lim = limit_ref.load(Ordering::Acquire);
                        if cur < lim {
                            if running_ref
                                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                break;
                            }
                        } else {
                            tokio::select! {
                                _ = slot_free_ref.notified() => {}
                                _ = cancel_token.cancelled() => {
                                    ffmpeg_pids_ref.write().await.remove(&job_id);
                                    jobs_ref.write().await.remove(&job_id);
                                    let _ = resp_tx.send(Response::JobCancelled { id: job_id.clone() }).await;
                                    return;
                                }
                            }
                        }
                    }

                    // Status auf Running setzen
                    {
                        let mut map = jobs_ref.write().await;
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
                            pid_slot,
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
                                    let mut map = jobs_ref.write().await;
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
                                    let mut map = jobs_ref.write().await;
                                    if let Some(j) = map.get_mut(&id) {
                                        j.status = JobState::Done;
                                        j.percent = 100.0;
                                    }
                                }
                                let _ = resp_tx.send(Response::JobDone { id }).await;
                            }
                            FfmpegEvent::Error { id, message } => {
                                {
                                    let mut map = jobs_ref.write().await;
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
                                    let mut map = jobs_ref.write().await;
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

                    match ffmpeg_handle.await {
                        Ok(Ok(())) => {}  // Normale Beendigung: terminales Event wurde bereits gesendet
                        Ok(Err(e)) => {
                            // run_ffmpeg schlug fehl ohne Event zu senden (z.B. ffmpeg nicht gefunden)
                            let _ = resp_tx.send(Response::JobError {
                                id: job_id.clone(),
                                message: format!("FFmpeg konnte nicht ausgefuehrt werden: {e}"),
                            }).await;
                        }
                        Err(e) => {
                            // FFmpeg-Task panikt (sehr selten)
                            let _ = resp_tx.send(Response::JobError {
                                id: job_id.clone(),
                                message: format!("FFmpeg-Task Panik: {e}"),
                            }).await;
                        }
                    }

                    // Slot freigeben und wartende Jobs benachrichtigen
                    running_ref.fetch_sub(1, Ordering::AcqRel);
                    slot_free_ref.notify_waiters();

                    // PID-Eintrag und Job aus HashMaps entfernen
                    ffmpeg_pids_ref.write().await.remove(&job_id);
                    jobs_ref.write().await.remove(&job_id);
                });

                // Monitor: wenn der Job-Task panikt → JobError an Python senden
                let monitor_tx = response_tx.clone();
                let monitor_id = job_id_for_monitor;
                tokio::spawn(async move {
                    if let Err(e) = handle.await {
                        let _ = monitor_tx.send(Response::JobError {
                            id: monitor_id,
                            message: format!("Job-Task-Panik: {e}"),
                        }).await;
                    }
                });
            }
            JobCommand::SetMaxParallel(n) => {
                limit.store(n.max(1), Ordering::Release);
                slot_free.notify_waiters();
            }
            JobCommand::PauseAll => {
                is_paused.store(true, Ordering::Release);
                let pids = ffmpeg_pids.read().await;
                for pid_slot in pids.values() {
                    let pid = pid_slot.load(Ordering::Acquire);
                    if pid != 0 {
                        unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP); }
                    }
                }
            }
            JobCommand::ResumeAll => {
                is_paused.store(false, Ordering::Release);
                let pids = ffmpeg_pids.read().await;
                for pid_slot in pids.values() {
                    let pid = pid_slot.load(Ordering::Acquire);
                    if pid != 0 {
                        unsafe { libc::kill(pid as libc::pid_t, libc::SIGCONT); }
                    }
                }
                drop(pids);
                slot_free.notify_waiters();
            }
            JobCommand::Cancel(id) => {
                let map = jobs.read().await;
                if let Some(job) = map.get(&id) {
                    job.cancel_token.cancel();
                }
            }
            JobCommand::GetStatus(reply) => {
                let mut map = jobs.write().await;
                // Alte abgeschlossene Jobs entfernen
                map.retain(|_, job| {
                    matches!(job.status, JobState::Running | JobState::Queued)
                });
                let statuses: Vec<JobStatus> = map.values().map(|j| j.to_status()).collect();
                let _ = reply.send(statuses);
            }
        }
    }
}

/// Ermittelt das Pixel-Format des ersten Video-Streams via ffprobe.
/// Gibt einen leeren String zurueck wenn das Format nicht ermittelt werden kann
/// (fuehrt dann zur sicheren Hybrid-Pipeline).
async fn probe_pix_fmt(path: &Path) -> String {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=pix_fmt",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.as_os_str())
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// Gibt true zurueck wenn NVDEC + scale_cuda das gegebene Pixel-Format unterstuetzen.
/// NVDEC unterstuetzt 4:2:0-Formate (8-bit und 10-bit); 4:2:2 (z.B. p210le von
/// Sony FX MXF) und andere exotische Formate erfordern die Hybrid-Pipeline.
fn nvenc_full_gpu_supported(pix_fmt: &str) -> bool {
    matches!(
        pix_fmt,
        "yuv420p" | "nv12" | "yuvj420p"
            | "yuv420p10le" | "yuv420p10be"
            | "p010le" | "p010be" | "p016le"
            | "yuv420p12le" | "p012le"
    )
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
