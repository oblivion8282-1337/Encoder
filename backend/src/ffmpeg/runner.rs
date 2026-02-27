// Startet FFmpeg-Prozesse ueber tokio::process::Command und liefert
// Fortschritts-Events ueber einen mpsc channel zurueck.

use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ffmpeg::progress::{calculate_progress, ProgressParser};
use crate::ipc::protocol::{JobMode, JobOptions};

/// Events die der FFmpeg-Runner an den Job-Manager sendet.
#[derive(Debug, Clone)]
pub enum FfmpegEvent {
    Progress {
        id: String,
        percent: f32,
        fps: f32,
        speed: f32,
        frame: u64,
    },
    Done {
        id: String,
    },
    Error {
        id: String,
        message: String,
    },
    Cancelled {
        id: String,
    },
}

/// Normalisiert eine Resolution-Angabe fuer FFmpeg.
/// "1920x1080" -> "1920:1080" (FFmpeg erwartet ':' als Trennzeichen).
fn normalize_resolution(res: &str) -> String {
    res.replace('x', ":")
}

/// Baut die FFmpeg-Argumente fuer den gegebenen Modus zusammen.
///
/// Argument-Reihenfolge ist kritisch:
/// 1. -y (overwrite)
/// 2. HW-Accel Flags VOR -i (vaapi_device, hwaccel cuda)
/// 3. -loglevel warning
/// 4. -i INPUT
/// 5. Mapping + Codec-Optionen
/// 6. -progress pipe:2
/// 7. OUTPUT
pub fn build_ffmpeg_args(
    input_path: &Path,
    output_path: &Path,
    mode: &JobMode,
    options: &JobOptions,
) -> Vec<String> {
    let mut args = Vec::new();

    // Overwrite ohne Nachfrage
    args.push("-y".to_string());

    // HW-Accel Flags VOR -i (nur bei Proxy)
    if matches!(mode, JobMode::Proxy) {
        match options.hw_accel.as_str() {
            "vaapi" => {
                args.push("-vaapi_device".to_string());
                args.push("/dev/dri/renderD128".to_string());
            }
            "nvenc" => {}

            _ => {}
        }
    }

    // Weniger stderr-Noise
    args.push("-loglevel".to_string());
    args.push("warning".to_string());

    // Input
    args.push("-i".to_string());
    args.push(input_path.to_string_lossy().to_string());

    // Mapping: erster Video-Stream, ALLE Audio-Streams (Sony FX 8-Kanal Mono)
    args.push("-map".to_string());
    args.push("0:v:0".to_string());
    args.push("-map".to_string());
    args.push("0:a".to_string());

    // Metadata und Timecode (tmcd/data track)
    args.push("-map_metadata".to_string());
    args.push("0".to_string());
    args.push("-map".to_string());
    args.push("0:d?".to_string());

    // Modus-spezifische Codecs
    match mode {
        JobMode::ReWrap => {
            args.push("-c:v".to_string());
            args.push("copy".to_string());
            args.push("-c:a".to_string());
            args.push(options.audio_codec.clone());
        }
        JobMode::Proxy => {
            match options.hw_accel.as_str() {
                "vaapi" => {
                    args.push("-c:v".to_string());
                    args.push("h264_vaapi".to_string());
                    args.push("-qp".to_string());
                    args.push("23".to_string());

                    // VAAPI braucht format=nv12,hwupload; Skalierung via scale_vaapi
                    args.push("-vf".to_string());
                    if let Some(ref resolution) = options.proxy_resolution {
                        let res = normalize_resolution(resolution);
                        args.push(format!("format=nv12,hwupload,scale_vaapi={res}"));
                    } else {
                        args.push("format=nv12,hwupload".to_string());
                    }
                }
                "nvenc" => {
                    args.push("-c:v".to_string());
                    args.push("h264_nvenc".to_string());
                    args.push("-preset".to_string());
                    args.push("p4".to_string());
                    args.push("-rc".to_string());
                    args.push("constqp".to_string());
                    args.push("-qp".to_string());
                    args.push("23".to_string());

                    // Skalierung falls gewuenscht (normaler scale-Filter)
                    if let Some(ref resolution) = options.proxy_resolution {
                        let res = normalize_resolution(resolution);
                        args.push("-vf".to_string());
                        args.push(format!("scale={res}"));
                    }
                }
                _ => {
                    // Software encoding (libx264)
                    args.push("-c:v".to_string());
                    args.push("libx264".to_string());
                    args.push("-crf".to_string());
                    args.push("23".to_string());
                    args.push("-preset".to_string());
                    args.push("fast".to_string());
                    args.push("-pix_fmt".to_string());
                    args.push("yuv420p".to_string());

                    // Skalierung falls gewuenscht
                    if let Some(ref resolution) = options.proxy_resolution {
                        let res = normalize_resolution(resolution);
                        args.push("-vf".to_string());
                        args.push(format!("scale={res}"));
                    }
                }
            }

            // Audio bei Proxy: pcm_s16le
            args.push("-c:a".to_string());
            args.push("pcm_s16le".to_string());
        }
    }

    // Strukturiertes Progress-Reporting auf stderr
    args.push("-progress".to_string());
    args.push("pipe:2".to_string());

    // Output
    args.push(output_path.to_string_lossy().to_string());

    args
}

/// Startet einen FFmpeg-Prozess und sendet Events ueber den Channel.
///
/// * `job_id` – Eindeutige Job-ID fuer die Events
/// * `args` – Komplette FFmpeg-Argumentliste
/// * `total_duration_us` – Gesamtdauer der Quelldatei in Mikrosekunden (fuer Prozentberechnung)
/// * `tx` – Channel fuer Events
/// * `cancel` – CancellationToken zum Abbrechen
pub async fn run_ffmpeg(
    job_id: String,
    args: Vec<String>,
    total_duration_us: i64,
    tx: mpsc::Sender<FfmpegEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("FFmpeg konnte nicht gestartet werden")?;

    let stderr = child
        .stderr
        .take()
        .context("Konnte stderr von FFmpeg nicht lesen")?;

    let mut stdin = child.stdin.take();

    let mut reader = BufReader::new(stderr).lines();
    let mut parser = ProgressParser::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Graceful stop: 'q' an stdin senden
                if let Some(ref mut stdin_handle) = stdin {
                    let _ = stdin_handle.write_all(b"q\n").await;
                    let _ = stdin_handle.flush().await;
                }
                let _ = child.wait().await;
                let _ = tx
                    .send(FfmpegEvent::Cancelled {
                        id: job_id.clone(),
                    })
                    .await;
                return Ok(());
            }
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(progress) = parser.feed_line(&line) {
                            if progress.is_done {
                                // Warten bis der Prozess beendet ist
                                let status = child.wait().await?;
                                if status.success() {
                                    let _ = tx
                                        .send(FfmpegEvent::Done { id: job_id.clone() })
                                        .await;
                                } else {
                                    let _ = tx
                                        .send(FfmpegEvent::Error {
                                            id: job_id.clone(),
                                            message: format!(
                                                "FFmpeg beendet mit Exit-Code: {}",
                                                status.code().unwrap_or(-1)
                                            ),
                                        })
                                        .await;
                                }
                                return Ok(());
                            }

                            let percent = calculate_progress(
                                progress.out_time_us,
                                total_duration_us,
                            );

                            let _ = tx
                                .send(FfmpegEvent::Progress {
                                    id: job_id.clone(),
                                    percent: percent * 100.0,
                                    fps: progress.fps,
                                    speed: progress.speed,
                                    frame: progress.frame,
                                })
                                .await;
                        }
                    }
                    Ok(None) => {
                        // stderr geschlossen – Prozess beendet
                        let status = child.wait().await?;
                        if status.success() {
                            let _ = tx
                                .send(FfmpegEvent::Done { id: job_id.clone() })
                                .await;
                        } else {
                            let _ = tx
                                .send(FfmpegEvent::Error {
                                    id: job_id.clone(),
                                    message: format!(
                                        "FFmpeg beendet mit Exit-Code: {}",
                                        status.code().unwrap_or(-1)
                                    ),
                                })
                                .await;
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;  // Zombie verhindern
                        let _ = tx
                            .send(FfmpegEvent::Error {
                                id: job_id.clone(),
                                message: format!("Fehler beim Lesen von stderr: {e}"),
                            })
                            .await;
                        return Ok(());
                    }
                }
            }
        }
    }
}
