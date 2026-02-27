// Einstiegspunkt des Rust-Backends.
// Startet den IPC-Server (stdin/stdout NDJSON) und die Job-Queue.

mod ffmpeg;
mod ipc;
mod jobs;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use ipc::protocol::Response;
use jobs::transcode::{self, JobQueue};

#[tokio::main]
async fn main() -> Result<()> {
    // Channel fuer Responses (von Job-Queue an stdout-Writer)
    let (response_tx, response_rx) = mpsc::channel::<Response>(256);

    // Job-Queue initialisieren (max 2 parallele Jobs)
    let max_parallel = 2;
    let (queue, cmd_rx) = JobQueue::new(max_parallel, response_tx.clone());
    let global_shutdown_token = queue.shutdown_token();
    let queue = Arc::new(queue);

    // Shutdown-Channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // stdout-Writer Task: Schreibt Response-Events als NDJSON
    let stdout_handle = tokio::spawn(ipc::server::write_stdout(response_rx));

    // Job-Queue Runner Task: Verarbeitet Job-Kommandos
    let queue_handle = tokio::spawn(transcode::run_queue(
        cmd_rx,
        max_parallel,
        response_tx.clone(),
        global_shutdown_token.clone(),
    ));

    // stdin-Reader Task: Liest Requests und dispatcht sie
    let stdin_handle = tokio::spawn(ipc::server::read_stdin(queue, response_tx.clone(), shutdown_tx));

    // Auf Shutdown warten (entweder via Shutdown-Request oder stdin EOF)
    tokio::select! {
        _ = shutdown_rx => {
            eprintln!("Shutdown-Signal empfangen, beende...");
        }
        result = stdin_handle => {
            if let Err(e) = result {
                eprintln!("stdin-Handler Fehler: {e}");
            }
        }
    }

    // Aufraemen: Alle laufenden FFmpeg-Prozesse via CancellationToken beenden
    global_shutdown_token.cancel();

    // Kurz warten damit laufende Jobs sauber beendet werden koennen
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Tasks beenden
    queue_handle.abort();
    stdout_handle.abort();

    Ok(())
}
