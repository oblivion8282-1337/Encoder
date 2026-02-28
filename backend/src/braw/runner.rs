// Startet braw-bridge + FFmpeg Pipeline fuer BRAW-Proxy-Generierung.
// braw-bridge liefert rawvideo rgb24 auf stdout und NDJSON-Events auf stderr.
// FFmpeg liest die rohen Frames von stdin und encodiert sie als Proxy.

use anyhow::{Context, Result};
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::runner::{push_proxy_codec_args, FfmpegEvent};
use crate::ipc::protocol::JobOptions;

/// Metadaten einer BRAW-Datei, geliefert von braw-bridge.
#[derive(Debug, Clone)]
pub struct BrawMetadata {
    pub timecode: String,
    pub fps_num: u32,
    pub fps_den: u32,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
}

/// Sucht das braw-bridge Binary in folgender Reihenfolge:
/// 1. BRAW_BRIDGE_PATH Umgebungsvariable
/// 2. Neben dem aktuellen Binary (gleicher Ordner wie std::env::current_exe())
/// 3. PATH (einfacher Name, wird vom OS aufgeloest)
fn find_braw_bridge() -> PathBuf {
    // 1. Env var
    if let Ok(path) = std::env::var("BRAW_BRIDGE_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
    }

    // 2. Neben aktuellem Binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("braw-bridge");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    // 3. PATH Fallback
    PathBuf::from("braw-bridge")
}

/// Ermittelt BRAW-Metadaten via `braw-bridge --input <file> --probe-only`.
/// Startet den Prozess, liest die erste stderr-Zeile (metadata JSON) und beendet sich.
pub async fn probe_braw_metadata(input_path: &Path) -> Result<BrawMetadata> {
    let bridge = find_braw_bridge();
    let output = Command::new(&bridge)
        .arg("--input")
        .arg(input_path.as_os_str())
        .arg("--probe-only")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("braw-bridge konnte nicht gestartet werden: {:?}", bridge))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let first_line = stderr
        .lines()
        .next()
        .context("braw-bridge hat keine Metadaten ausgegeben")?;

    parse_metadata_json(first_line)
}

/// Parst eine JSON-Zeile mit BRAW-Metadaten.
fn parse_metadata_json(json_str: &str) -> Result<BrawMetadata> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).context("braw-bridge Metadaten-JSON ungueltig")?;

    Ok(BrawMetadata {
        timecode: v["timecode"]
            .as_str()
            .unwrap_or("00:00:00:00")
            .to_string(),
        fps_num: v["fps_num"]
            .as_u64()
            .context("fps_num fehlt in Metadaten")? as u32,
        fps_den: v["fps_den"]
            .as_u64()
            .context("fps_den fehlt in Metadaten")? as u32,
        width: v["width"]
            .as_u64()
            .context("width fehlt in Metadaten")? as u32,
        height: v["height"]
            .as_u64()
            .context("height fehlt in Metadaten")? as u32,
        frame_count: v["frame_count"]
            .as_u64()
            .context("frame_count fehlt in Metadaten")?,
    })
}

/// Baut FFmpeg-Argumente fuer BRAW-Proxy-Encoding.
/// Input ist rawvideo rgb24 von stdin (pipe:0), kein HW-Accel fuer Decode.
fn build_braw_ffmpeg_args(
    output_path: &Path,
    options: &JobOptions,
    meta: &BrawMetadata,
) -> Vec<String> {
    let mut args = Vec::new();

    // Overwrite
    args.push("-y".to_string());

    // Weniger stderr-Noise
    args.push("-loglevel".to_string());
    args.push("warning".to_string());

    // Raw-Video Input von stdin
    args.push("-f".to_string());
    args.push("rawvideo".to_string());
    args.push("-pix_fmt".to_string());
    args.push("rgb24".to_string());
    args.push("-s".to_string());
    args.push(format!("{}x{}", meta.width, meta.height));
    args.push("-r".to_string());
    args.push(format!("{}/{}", meta.fps_num, meta.fps_den));
    args.push("-i".to_string());
    args.push("pipe:0".to_string());

    // Proxy-Codec-Argumente (gleiche Logik wie normaler Proxy, aber kein HW-Accel fuer Input)
    let resolution = options
        .proxy_resolution
        .as_deref()
        .map(|r| r.replace('x', ":"));
    // BRAW: kein HW-Accel (rawvideo von Pipe), kein NVDEC
    push_proxy_codec_args(
        &mut args,
        &options.proxy_codec,
        "none",
        resolution.as_deref(),
        false,
    );

    // Audio: kein Audio bei BRAW-Proxies (BRAW enthaelt kein Audio)

    // Timecode als Container-Metadaten
    if !meta.timecode.is_empty() {
        args.push("-metadata".to_string());
        args.push(format!("timecode={}", meta.timecode));
    }

    // Output
    args.push(output_path.to_string_lossy().to_string());

    args
}

/// Startet braw-bridge + FFmpeg Pipeline und sendet Events ueber den Channel.
///
/// braw-bridge stdout (rawvideo rgb24) wird direkt an FFmpeg stdin gepiped.
/// braw-bridge stderr liefert NDJSON progress-Events.
pub async fn run_braw_job(
    job_id: String,
    input_path: PathBuf,
    output_path: PathBuf,
    options: &JobOptions,
    meta: BrawMetadata,
    tx: mpsc::Sender<FfmpegEvent>,
    cancel: CancellationToken,
    pid_slot: Arc<AtomicU32>,
) -> Result<()> {
    let bridge = find_braw_bridge();
    let ffmpeg_args = build_braw_ffmpeg_args(&output_path, options, &meta);

    // braw-bridge starten
    let mut bridge_child = Command::new(&bridge)
        .arg("--input")
        .arg(input_path.as_os_str())
        .arg("--debayer")
        .arg(&options.debayer_quality)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("braw-bridge konnte nicht gestartet werden: {:?}", bridge))?;

    // PID von braw-bridge speichern (fuer Pause/Resume SIGSTOP/SIGCONT)
    pid_slot.store(bridge_child.id().unwrap_or(0), Ordering::Release);

    let bridge_stdout = bridge_child
        .stdout
        .take()
        .context("Konnte stdout von braw-bridge nicht lesen")?;

    let bridge_stderr = bridge_child
        .stderr
        .take()
        .context("Konnte stderr von braw-bridge nicht lesen")?;

    // Erste stderr-Zeile lesen: Metadaten (bereits in `meta` vorhanden, ueberspringen)
    let mut stderr_reader = BufReader::new(bridge_stderr).lines();
    let first_line = stderr_reader.next_line().await?;
    if first_line.is_none() {
        let _ = tx
            .send(FfmpegEvent::Error {
                id: job_id,
                message: "braw-bridge hat keine Metadaten-Zeile ausgegeben".to_string(),
            })
            .await;
        return Ok(());
    }

    // FFmpeg starten mit braw-bridge stdout als stdin.
    // tokio::ChildStdout → OwnedFd → std::process::Stdio
    let bridge_stdout_raw: std::process::Stdio = {
        let owned_fd = bridge_stdout.into_owned_fd()
            .context("Konnte braw-bridge stdout FD nicht uebernehmen")?;
        unsafe { std::process::Stdio::from_raw_fd(owned_fd.into_raw_fd()) }
    };
    let mut ffmpeg_child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(bridge_stdout_raw)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("FFmpeg konnte nicht gestartet werden")?;

    let total_frames = meta.frame_count;

    // Event-Loop: braw-bridge stderr lesen fuer Progress, Cancel abfangen
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // braw-bridge mit SIGTERM killen → pipe bricht ab → ffmpeg stoppt
                let bridge_pid = pid_slot.load(Ordering::Acquire);
                if bridge_pid != 0 {
                    unsafe { libc::kill(bridge_pid as libc::pid_t, libc::SIGTERM); }
                }
                let _ = bridge_child.wait().await;
                let _ = ffmpeg_child.wait().await;
                pid_slot.store(0, Ordering::Release);
                let _ = tx
                    .send(FfmpegEvent::Cancelled {
                        id: job_id.clone(),
                    })
                    .await;
                return Ok(());
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        // Progress-Events parsen: {"type":"progress","frame":42,"total":1200}
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                            if v["type"].as_str() == Some("progress") {
                                let frame = v["frame"].as_u64().unwrap_or(0);
                                let percent = if total_frames > 0 {
                                    (frame as f32 / total_frames as f32 * 100.0).clamp(0.0, 100.0)
                                } else {
                                    0.0
                                };
                                let _ = tx
                                    .send(FfmpegEvent::Progress {
                                        id: job_id.clone(),
                                        percent,
                                        fps: 0.0,
                                        speed: 0.0,
                                        frame,
                                    })
                                    .await;
                            }
                        }
                    }
                    Ok(None) => {
                        // stderr geschlossen – braw-bridge beendet
                        let bridge_status = bridge_child.wait().await?;
                        let ffmpeg_status = ffmpeg_child.wait().await?;
                        pid_slot.store(0, Ordering::Release);

                        if !bridge_status.success() {
                            let _ = tx
                                .send(FfmpegEvent::Error {
                                    id: job_id.clone(),
                                    message: format!(
                                        "braw-bridge beendet mit Exit-Code: {}",
                                        bridge_status.code().unwrap_or(-1)
                                    ),
                                })
                                .await;
                        } else if !ffmpeg_status.success() {
                            let _ = tx
                                .send(FfmpegEvent::Error {
                                    id: job_id.clone(),
                                    message: format!(
                                        "FFmpeg beendet mit Exit-Code: {}",
                                        ffmpeg_status.code().unwrap_or(-1)
                                    ),
                                })
                                .await;
                        } else {
                            let _ = tx
                                .send(FfmpegEvent::Done { id: job_id.clone() })
                                .await;
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        // Lesefehler – beide Prozesse killen
                        let _ = bridge_child.kill().await;
                        let _ = bridge_child.wait().await;
                        let _ = ffmpeg_child.kill().await;
                        let _ = ffmpeg_child.wait().await;
                        pid_slot.store(0, Ordering::Release);
                        let _ = tx
                            .send(FfmpegEvent::Error {
                                id: job_id.clone(),
                                message: format!("Fehler beim Lesen von braw-bridge stderr: {e}"),
                            })
                            .await;
                        return Ok(());
                    }
                }
            }
        }
    }
}
