// Startet r3d-bridge + FFmpeg Pipeline fuer R3D-Proxy-Generierung.
// r3d-bridge liefert rawvideo rgb24 auf stdout und NDJSON-Events auf stderr.
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

use crate::ffmpeg::runner::{is_prores, push_proxy_codec_args, FfmpegEvent};
use crate::ipc::protocol::JobOptions;

/// Metadaten einer R3D-Datei, geliefert von r3d-bridge.
#[derive(Debug, Clone)]
pub struct R3dMetadata {
    pub timecode: String,
    pub fps_num: u32,
    pub fps_den: u32,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
}

/// Sucht das r3d-bridge Binary in folgender Reihenfolge:
/// 1. R3D_BRIDGE_PATH Umgebungsvariable
/// 2. Neben dem aktuellen Binary (gleicher Ordner wie std::env::current_exe())
/// 3. PATH (einfacher Name, wird vom OS aufgeloest)
fn find_r3d_bridge() -> PathBuf {
    // 1. Env var
    if let Ok(path) = std::env::var("R3D_BRIDGE_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
    }

    // 2. Neben aktuellem Binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("r3d-bridge");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    // 3. PATH Fallback
    PathBuf::from("r3d-bridge")
}

/// Ermittelt R3D-Metadaten via `r3d-bridge --input <file> --probe-only`.
pub async fn probe_r3d_metadata(input_path: &Path) -> Result<R3dMetadata> {
    let bridge = find_r3d_bridge();
    let output = Command::new(&bridge)
        .arg("--input")
        .arg(input_path.as_os_str())
        .arg("--probe-only")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("r3d-bridge konnte nicht gestartet werden: {:?}", bridge))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let first_line = stderr
        .lines()
        .next()
        .context("r3d-bridge hat keine Metadaten ausgegeben")?;

    parse_metadata_json(first_line)
}

/// Parst eine JSON-Zeile mit R3D-Metadaten.
fn parse_metadata_json(json_str: &str) -> Result<R3dMetadata> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).context("r3d-bridge Metadaten-JSON ungueltig")?;

    Ok(R3dMetadata {
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

/// Extrahiert Audio aus einer R3D-Datei als temporaere WAV-Datei.
/// Gibt den Pfad zur WAV-Datei zurueck, oder None wenn kein Audio vorhanden.
async fn extract_r3d_audio(bridge: &Path, input_path: &Path, job_id: &str) -> Option<PathBuf> {
    let wav_path = std::env::temp_dir().join(format!("proxy-gen-r3d-audio-{}.wav", job_id));
    let status = Command::new(bridge)
        .arg("--input")
        .arg(input_path.as_os_str())
        .arg("--extract-audio")
        .arg(&wav_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .ok()?;
    if status.success() && wav_path.exists() {
        Some(wav_path)
    } else {
        None
    }
}

/// Baut FFmpeg-Argumente fuer R3D-Proxy-Encoding.
/// Input ist rawvideo rgb24 von stdin (pipe:0).
/// Optional: audio_path fuer einen zweiten WAV-Input.
fn build_r3d_ffmpeg_args(
    output_path: &Path,
    options: &JobOptions,
    meta: &R3dMetadata,
    audio_path: Option<&Path>,
) -> Vec<String> {
    let mut args = Vec::new();

    // Overwrite
    args.push("-y".to_string());

    // Weniger stderr-Noise
    args.push("-loglevel".to_string());
    args.push("warning".to_string());

    // Frame-Dimensionen nach Debayer berechnen.
    // probe_r3d_metadata liefert die volle Sensor-Aufloesung;
    // r3d-bridge gibt bei half/quarter/eighth entsprechend kleinere Frames aus.
    let (frame_width, frame_height) = match options.r3d_debayer_quality.to_lowercase().as_str() {
        "half"    => (meta.width / 2, meta.height / 2),
        "quarter" => (meta.width / 4, meta.height / 4),
        "eighth"  => (meta.width / 8, meta.height / 8),
        _         => (meta.width, meta.height), // "premium" oder default = volle Aufloesung
    };

    // HW-Accel Init-Flags VOR -i (nur fuer GPU-Encoder, nicht fuer ProRes)
    if !is_prores(&options.proxy_codec) {
        match options.hw_accel.as_str() {
            "nvenc" => {
                args.push("-init_hw_device".to_string());
                args.push("cuda=cuda:0".to_string());
                args.push("-filter_hw_device".to_string());
                args.push("cuda".to_string());
            }
            "vaapi" => {
                args.push("-vaapi_device".to_string());
                args.push("/dev/dri/renderD128".to_string());
            }
            _ => {}
        }
    }

    // Input 0: Raw-Video von stdin
    args.push("-f".to_string());
    args.push("rawvideo".to_string());
    args.push("-pix_fmt".to_string());
    args.push("rgb24".to_string());
    args.push("-s".to_string());
    args.push(format!("{}x{}", frame_width, frame_height));
    args.push("-r".to_string());
    args.push(format!("{}/{}", meta.fps_num, meta.fps_den));
    args.push("-i".to_string());
    args.push("pipe:0".to_string());

    // Input 1: Audio-WAV (optional)
    if let Some(wav) = audio_path {
        args.push("-i".to_string());
        args.push(wav.to_string_lossy().to_string());
    }

    // Stream-Mapping
    if audio_path.is_some() {
        args.push("-map".to_string());
        args.push("0:v:0".to_string());
        args.push("-map".to_string());
        args.push("1:a".to_string());
    }

    // Video-Codec
    let resolution = options
        .proxy_resolution
        .as_deref()
        .map(|r| r.replace('x', ":"));
    push_proxy_codec_args(
        &mut args,
        &options.proxy_codec,
        &options.hw_accel,
        resolution.as_deref(),
        false, // full_gpu=false: kein NVDEC moeglich, CPU-Decode → GPU-Encode
    );

    // Audio-Codec (PCM, nur wenn Audio vorhanden)
    if audio_path.is_some() {
        args.push("-c:a".to_string());
        args.push("pcm_s32le".to_string());
    }

    // Timecode als Container-Metadaten
    if !meta.timecode.is_empty() {
        args.push("-metadata".to_string());
        args.push(format!("timecode={}", meta.timecode));
    }

    // Output
    args.push(output_path.to_string_lossy().to_string());

    args
}

/// Startet r3d-bridge + FFmpeg Pipeline und sendet Events ueber den Channel.
///
/// Ablauf:
/// 1. Audio-Extraktion (r3d-bridge --extract-audio) in temp-WAV
/// 2. r3d-bridge stdout (rawvideo rgb24) → FFmpeg stdin
/// 3. FFmpeg muxed Video + Audio (falls vorhanden) in Proxy
/// 4. Temp-WAV wird nach Abschluss geloescht
pub async fn run_r3d_job(
    job_id: String,
    input_path: PathBuf,
    output_path: PathBuf,
    options: &JobOptions,
    meta: R3dMetadata,
    tx: mpsc::Sender<FfmpegEvent>,
    cancel: CancellationToken,
    pid_slot: Arc<AtomicU32>,
) -> Result<()> {
    let bridge = find_r3d_bridge();

    // Schritt 1: Audio extrahieren
    let audio_wav = extract_r3d_audio(&bridge, &input_path, &job_id).await;

    let ffmpeg_args = build_r3d_ffmpeg_args(&output_path, options, &meta, audio_wav.as_deref());

    // Schritt 2: r3d-bridge starten
    let debayer_arg = options.r3d_debayer_quality.to_lowercase();
    let mut bridge_child = Command::new(&bridge)
        .arg("--input")
        .arg(input_path.as_os_str())
        .arg("--debayer")
        .arg(&debayer_arg)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("r3d-bridge konnte nicht gestartet werden: {:?}", bridge))?;

    // PID von r3d-bridge speichern (fuer Pause/Resume SIGSTOP/SIGCONT)
    pid_slot.store(bridge_child.id().unwrap_or(0), Ordering::Release);

    let bridge_stdout = bridge_child
        .stdout
        .take()
        .context("Konnte stdout von r3d-bridge nicht lesen")?;

    let bridge_stderr = bridge_child
        .stderr
        .take()
        .context("Konnte stderr von r3d-bridge nicht lesen")?;

    // Erste stderr-Zeile lesen: Metadaten (bereits in `meta` vorhanden, ueberspringen)
    let mut stderr_reader = BufReader::new(bridge_stderr).lines();
    let first_line = stderr_reader.next_line().await?;
    if first_line.is_none() {
        cleanup_audio(&audio_wav);
        let _ = tx
            .send(FfmpegEvent::Error {
                id: job_id,
                message: "r3d-bridge hat keine Metadaten-Zeile ausgegeben".to_string(),
            })
            .await;
        return Ok(());
    }

    // Schritt 3: FFmpeg starten mit r3d-bridge stdout als stdin.
    let bridge_stdout_raw: std::process::Stdio = {
        let owned_fd = bridge_stdout.into_owned_fd()
            .context("Konnte r3d-bridge stdout FD nicht uebernehmen")?;
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

    // Event-Loop: r3d-bridge stderr lesen fuer Progress, Cancel abfangen
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // r3d-bridge mit SIGTERM killen → pipe bricht ab → ffmpeg stoppt
                let bridge_pid = pid_slot.load(Ordering::Acquire);
                if bridge_pid != 0 {
                    unsafe { libc::kill(bridge_pid as libc::pid_t, libc::SIGTERM); }
                }
                let _ = bridge_child.wait().await;
                let _ = ffmpeg_child.wait().await;
                pid_slot.store(0, Ordering::Release);
                cleanup_audio(&audio_wav);
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
                        // Progress-Events parsen
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
                        // stderr geschlossen – r3d-bridge beendet
                        let bridge_status = bridge_child.wait().await?;
                        let ffmpeg_status = ffmpeg_child.wait().await?;
                        pid_slot.store(0, Ordering::Release);
                        cleanup_audio(&audio_wav);

                        if !bridge_status.success() {
                            let _ = tx
                                .send(FfmpegEvent::Error {
                                    id: job_id.clone(),
                                    message: format!(
                                        "r3d-bridge beendet mit Exit-Code: {}",
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
                        let _ = bridge_child.kill().await;
                        let _ = bridge_child.wait().await;
                        let _ = ffmpeg_child.kill().await;
                        let _ = ffmpeg_child.wait().await;
                        pid_slot.store(0, Ordering::Release);
                        cleanup_audio(&audio_wav);
                        let _ = tx
                            .send(FfmpegEvent::Error {
                                id: job_id.clone(),
                                message: format!("Fehler beim Lesen von r3d-bridge stderr: {e}"),
                            })
                            .await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Loescht die temporaere Audio-WAV-Datei, falls vorhanden.
fn cleanup_audio(audio_wav: &Option<PathBuf>) {
    if let Some(path) = audio_wav {
        let _ = std::fs::remove_file(path);
    }
}
