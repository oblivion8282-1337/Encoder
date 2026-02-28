// Startet FFmpeg-Prozesse ueber tokio::process::Command und liefert
// Fortschritts-Events ueber einen mpsc channel zurueck.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
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
/// 2. HW-Accel Flags VOR -i (VAAPI-Device / CUDA-hwaccel)
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
    nvenc_full_gpu: bool,
) -> Vec<String> {
    let mut args = Vec::new();

    // Overwrite ohne Nachfrage
    args.push("-y".to_string());

    // HW-Accel Flags VOR -i (nur Proxy, nicht ProRes – ProRes ist immer CPU)
    if matches!(mode, JobMode::Proxy) && !is_prores(&options.proxy_codec) {
        match options.hw_accel.as_str() {
            "vaapi" => {
                args.push("-vaapi_device".to_string());
                args.push("/dev/dri/renderD128".to_string());
            }
            "nvenc" => {
                // CUDA-Device fuer Filtergraph (benoetigt von hwupload + scale_cuda).
                args.push("-init_hw_device".to_string());
                args.push("cuda=cuda:0".to_string());
                args.push("-filter_hw_device".to_string());
                args.push("cuda".to_string());
                if nvenc_full_gpu {
                    // Volle GPU-Pipeline: NVDEC dekodiert direkt in den GPU-Speicher.
                    // Frames bleiben auf der GPU – kein PCIe-Transfer noetig.
                    args.push("-hwaccel".to_string());
                    args.push("cuda".to_string());
                    args.push("-hwaccel_device".to_string());
                    args.push("cuda".to_string());
                    args.push("-hwaccel_output_format".to_string());
                    args.push("cuda".to_string());
                }
                // Ohne nvenc_full_gpu: CPU-Decode → format=nv12 → hwupload → scale_cuda.
                // Wird fuer Formate gewaehlt, die NVDEC nicht unterstuetzt (z.B. p210le).
            }
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

    // Globale Metadaten uebernehmen (Timecode etc. auf Container-Ebene)
    // Hinweis: -map 0:d? entfernt – Sony FX MXF hat smpte_436m_anc (nicht MOV-kompatibel)
    args.push("-map_metadata".to_string());
    args.push("0".to_string());

    // Modus-spezifische Codecs
    match mode {
        JobMode::ReWrap => {
            args.push("-c:v".to_string());
            args.push("copy".to_string());
            args.push("-c:a".to_string());
            args.push(options.audio_codec.clone());
        }
        JobMode::Proxy => {
            let res = options
                .proxy_resolution
                .as_deref()
                .map(normalize_resolution);
            push_proxy_codec_args(&mut args, &options.proxy_codec, &options.hw_accel, res.as_deref(), nvenc_full_gpu);

            // Audio bei Proxy: pcm_s16le
            args.push("-c:a".to_string());
            args.push("pcm_s16le".to_string());
        }
        JobMode::BrawProxy => {
            // BrawProxy wird nicht ueber build_ffmpeg_args abgewickelt –
            // eigene Arg-Logik in braw::runner::build_braw_ffmpeg_args
            unreachable!("BrawProxy nutzt eigene FFmpeg-Args via braw::runner");
        }
        JobMode::R3dProxy => {
            // R3dProxy wird nicht ueber build_ffmpeg_args abgewickelt –
            // eigene Arg-Logik in r3d::runner::build_r3d_ffmpeg_args
            unreachable!("R3dProxy nutzt eigene FFmpeg-Args via r3d::runner");
        }
    }

    // Strukturiertes Progress-Reporting auf stderr
    args.push("-progress".to_string());
    args.push("pipe:2".to_string());

    // Output
    args.push(output_path.to_string_lossy().to_string());

    args
}

// ---------------------------------------------------------------------------
// Codec-Hilfsfunktionen
// ---------------------------------------------------------------------------

pub fn is_prores(codec: &str) -> bool {
    matches!(codec, "prores_proxy" | "prores_lt" | "prores_422" | "prores_hq")
}

/// Waehlt den passenden Video-Encoder anhand von proxy_codec × hw_accel.
pub fn push_proxy_codec_args(
    args: &mut Vec<String>,
    proxy_codec: &str,
    hw_accel: &str,
    resolution: Option<&str>,
    nvenc_full_gpu: bool,
) {
    match proxy_codec {
        // ── H.264 ──────────────────────────────────────────────────────────
        "h264" => match hw_accel {
            "vaapi" => push_vaapi(args, "h264_vaapi", resolution),
            "nvenc" => push_nvenc(args, "h264_nvenc", "23", resolution, nvenc_full_gpu),
            _       => push_sw_x264(args, resolution),
        },
        // ── H.265 / HEVC ───────────────────────────────────────────────────
        "h265" => match hw_accel {
            "vaapi" => push_vaapi(args, "hevc_vaapi", resolution),
            "nvenc" => push_nvenc(args, "hevc_nvenc", "23", resolution, nvenc_full_gpu),
            _       => push_sw_x265(args, resolution),
        },
        // ── AV1 ────────────────────────────────────────────────────────────
        "av1" => match hw_accel {
            "vaapi" => push_vaapi(args, "av1_vaapi", resolution),
            // AV1 NVENC: braucht yuv420p (keine CUDA-Frames) + SW-scale statt scale_cuda
            "nvenc" => push_nvenc_av1(args, resolution),
            _       => push_sw_av1(args, resolution),
        },
        // ── ProRes ─────────────────────────────────────────────────────────
        c if is_prores(c) => push_prores(args, c, resolution),
        // ── Fallback: libx264 ──────────────────────────────────────────────
        _ => push_sw_x264(args, resolution),
    }
}

/// VAAPI-Encoder (h264_vaapi / hevc_vaapi / av1_vaapi).
/// Benoetigt format=nv12,hwupload fuer den Video-Filter.
fn push_vaapi(args: &mut Vec<String>, codec: &str, resolution: Option<&str>) {
    args.push("-c:v".to_string());
    args.push(codec.to_string());
    args.push("-rc_mode".to_string());
    args.push("CQP".to_string());
    args.push("-qp".to_string());
    args.push("23".to_string());
    args.push("-vf".to_string());
    match resolution {
        Some(res) => args.push(format!("format=nv12,hwupload,scale_vaapi={res}")),
        None      => args.push("format=nv12,hwupload".to_string()),
    }
}

/// NVENC-Encoder (h264_nvenc / hevc_nvenc).
/// CPU-Decode → format=nv12 (beliebiges Eingangsformat) → hwupload (CUDA) →
/// scale_cuda (GPU-Skalierung) → NVENC-Encode.
fn push_nvenc(args: &mut Vec<String>, codec: &str, qp: &str, resolution: Option<&str>, full_gpu: bool) {
    args.push("-c:v".to_string());
    args.push(codec.to_string());
    args.push("-preset".to_string());
    args.push("p4".to_string());
    args.push("-rc".to_string());
    args.push("constqp".to_string());
    args.push("-qp".to_string());
    args.push(qp.to_string());
    if full_gpu {
        // CUDA-Frames direkt von NVDEC → scale_cuda → NVENC, kein PCIe-Transfer.
        if let Some(res) = resolution {
            args.push("-vf".to_string());
            args.push(format!("scale_cuda={res}"));
        }
        // Ohne Skalierung: CUDA-Frames gehen direkt an NVENC, kein -vf noetig.
    } else {
        // Hybrid: CPU-Decode → format=nv12 (konvertiert auch p210le etc.) →
        // hwupload → scale_cuda → NVENC.
        if let Some(res) = resolution {
            args.push("-vf".to_string());
            args.push(format!("format=nv12,hwupload,scale_cuda={res}"));
        } else {
            // Kein Scale: format=nv12 konvertiert RGB/YUV → nv12,
            // hwupload laedt die Frames in den CUDA-Speicher fuer NVENC.
            args.push("-vf".to_string());
            args.push("format=nv12,hwupload".to_string());
        }
    }
}

/// AV1 NVENC: braucht Systemspeicher-Frames (kein CUDA-Input) und yuv420p.
/// Skalierung daher via Software-scale, nicht scale_cuda.
fn push_nvenc_av1(args: &mut Vec<String>, resolution: Option<&str>) {
    args.push("-c:v".to_string());
    args.push("av1_nvenc".to_string());
    args.push("-preset".to_string());
    args.push("p4".to_string());
    args.push("-rc".to_string());
    args.push("constqp".to_string());
    args.push("-qp".to_string());
    args.push("63".to_string());
    args.push("-pix_fmt".to_string());
    args.push("yuv420p".to_string());
    if let Some(res) = resolution {
        args.push("-vf".to_string());
        args.push(format!("scale={res}"));
    }
}

/// Software H.264 (libx264).
fn push_sw_x264(args: &mut Vec<String>, resolution: Option<&str>) {
    args.push("-c:v".to_string());
    args.push("libx264".to_string());
    args.push("-crf".to_string());
    args.push("23".to_string());
    args.push("-preset".to_string());
    args.push("fast".to_string());
    args.push("-pix_fmt".to_string());
    args.push("yuv420p".to_string());
    if let Some(res) = resolution {
        args.push("-vf".to_string());
        args.push(format!("scale={res}"));
    }
}

/// Software H.265 (libx265).
fn push_sw_x265(args: &mut Vec<String>, resolution: Option<&str>) {
    args.push("-c:v".to_string());
    args.push("libx265".to_string());
    args.push("-crf".to_string());
    args.push("23".to_string());
    args.push("-preset".to_string());
    args.push("fast".to_string());
    args.push("-pix_fmt".to_string());
    args.push("yuv420p".to_string());
    if let Some(res) = resolution {
        args.push("-vf".to_string());
        args.push(format!("scale={res}"));
    }
}

/// Software AV1 (libsvtav1 – schnellster freier AV1-Encoder).
fn push_sw_av1(args: &mut Vec<String>, resolution: Option<&str>) {
    args.push("-c:v".to_string());
    args.push("libsvtav1".to_string());
    args.push("-crf".to_string());
    args.push("30".to_string());
    args.push("-preset".to_string());
    args.push("8".to_string());
    args.push("-pix_fmt".to_string());
    args.push("yuv420p".to_string());
    if let Some(res) = resolution {
        args.push("-vf".to_string());
        args.push(format!("scale={res}"));
    }
}

/// Apple ProRes (prores_ks) – immer CPU, profile bestimmt Qualitaetsstufe.
fn push_prores(args: &mut Vec<String>, codec: &str, resolution: Option<&str>) {
    let profile = match codec {
        "prores_proxy" => "0",
        "prores_lt"    => "1",
        "prores_422"   => "2",
        "prores_hq"    => "3",
        _              => "2",
    };
    args.push("-c:v".to_string());
    args.push("prores_ks".to_string());
    args.push("-profile:v".to_string());
    args.push(profile.to_string());
    args.push("-pix_fmt".to_string());
    args.push("yuv422p10le".to_string());
    if let Some(res) = resolution {
        args.push("-vf".to_string());
        args.push(format!("scale={res}"));
    }
}

/// Startet einen FFmpeg-Prozess und sendet Events ueber den Channel.
///
/// * `job_id` – Eindeutige Job-ID fuer die Events
/// * `args` – Komplette FFmpeg-Argumentliste
/// * `output_path` – Pfad zur Output-Datei; wird bei Fehler-Exit geloescht (partial file cleanup)
/// * `total_duration_us` – Gesamtdauer der Quelldatei in Mikrosekunden (fuer Prozentberechnung)
/// * `tx` – Channel fuer Events
/// * `cancel` – CancellationToken zum Abbrechen
pub async fn run_ffmpeg(
    job_id: String,
    args: Vec<String>,
    output_path: &Path,
    total_duration_us: i64,
    tx: mpsc::Sender<FfmpegEvent>,
    cancel: CancellationToken,
    pid_slot: Arc<AtomicU32>,
) -> Result<()> {
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("FFmpeg konnte nicht gestartet werden")?;

    // PID registrieren damit Pause/Resume den Prozess signalisieren kann
    pid_slot.store(child.id().unwrap_or(0), Ordering::Release);

    let stderr = child
        .stderr
        .take()
        .context("Konnte stderr von FFmpeg nicht lesen")?;

    let mut stdin = child.stdin.take();

    let mut reader = BufReader::new(stderr).lines();
    let mut parser = ProgressParser::new();
    // Letzte Zeilen aus FFmpeg-stderr fuer Fehlermeldungen (max. 20)
    let mut log_tail: Vec<String> = Vec::with_capacity(20);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Graceful stop: 'q' an stdin senden
                if let Some(ref mut stdin_handle) = stdin {
                    let _ = stdin_handle.write_all(b"q\n").await;
                    let _ = stdin_handle.flush().await;
                }
                let _ = child.wait().await;
                pid_slot.store(0, Ordering::Release);
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
                                pid_slot.store(0, Ordering::Release);
                                if status.success() {
                                    let _ = tx
                                        .send(FfmpegEvent::Done { id: job_id.clone() })
                                        .await;
                                } else {
                                    let _ = std::fs::remove_file(output_path); // partial file cleanup
                                    let _ = tx
                                        .send(FfmpegEvent::Error {
                                            id: job_id.clone(),
                                            message: build_error_message(
                                                status.code().unwrap_or(-1),
                                                &log_tail,
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
                        } else {
                            // Keine Progress-Zeile → FFmpeg-Lognachricht puffern
                            if log_tail.len() == 20 {
                                log_tail.remove(0);
                            }
                            log_tail.push(line);
                        }
                    }
                    Ok(None) => {
                        // stderr geschlossen – Prozess beendet
                        let status = child.wait().await?;
                        pid_slot.store(0, Ordering::Release);
                        if status.success() {
                            let _ = tx
                                .send(FfmpegEvent::Done { id: job_id.clone() })
                                .await;
                        } else {
                            let _ = std::fs::remove_file(output_path); // partial file cleanup
                            let _ = tx
                                .send(FfmpegEvent::Error {
                                    id: job_id.clone(),
                                    message: build_error_message(
                                        status.code().unwrap_or(-1),
                                        &log_tail,
                                    ),
                                })
                                .await;
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;  // Zombie verhindern
                        pid_slot.store(0, Ordering::Release);
                        let _ = std::fs::remove_file(output_path); // partial file cleanup
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

/// Baut eine lesbare Fehlermeldung mit FFmpeg-Logausgabe.
fn build_error_message(exit_code: i32, log_tail: &[String]) -> String {
    if log_tail.is_empty() {
        return format!("FFmpeg beendet mit Exit-Code: {exit_code}");
    }
    format!(
        "FFmpeg beendet mit Exit-Code: {exit_code}\n\n{}",
        log_tail.join("\n")
    )
}
