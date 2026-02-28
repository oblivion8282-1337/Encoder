// Einstiegspunkt des Rust-Backends.
// Startet den IPC-Server (stdin/stdout NDJSON) und die Job-Queue.

mod braw;
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

    // max_parallel aus CLI-Argument lesen (--max-parallel N), Fallback: 1
    let max_parallel = std::env::args()
        .skip_while(|a| a != "--max-parallel")
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    let (queue, cmd_rx) = JobQueue::new(max_parallel, response_tx.clone());
    let global_shutdown_token = queue.shutdown_token();
    let queue = Arc::new(queue);

    // Shutdown-Channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // stdout-Writer Task: Schreibt Response-Events als NDJSON
    let stdout_handle = tokio::spawn(ipc::server::write_stdout(response_rx));

    // Job-Queue Runner Task: Verarbeitet Job-Kommandos
    let queue_resp_tx = response_tx.clone();
    let queue_handle = tokio::spawn(transcode::run_queue(
        cmd_rx,
        max_parallel,
        queue_resp_tx,
        global_shutdown_token.clone(),
    ));

    // stdin-Reader Task: Liest Requests und dispatcht sie
    let stdin_resp_tx = response_tx.clone();
    let stdin_handle = tokio::spawn(ipc::server::read_stdin(queue.clone(), stdin_resp_tx, shutdown_tx));
    let stdin_abort = stdin_handle.abort_handle();

    // stdout-Writer AbortHandle fuer spaetere Bereinigung
    let stdout_abort = stdout_handle.abort_handle();

    // Auf Shutdown warten (entweder via Shutdown-Request, stdin EOF, oder stdout-Fehler)
    tokio::select! {
        _ = shutdown_rx => {
            eprintln!("Shutdown-Signal empfangen, beende...");
        }
        result = stdin_handle => {
            if let Err(e) = result {
                eprintln!("stdin-Handler Fehler: {e}");
            }
        }
        result = stdout_handle => {
            match result {
                Ok(Err(e)) => eprintln!("stdout-Writer Fehler: {e} — beende..."),
                Err(e) => eprintln!("stdout-Writer Task Fehler: {e} — beende..."),
                Ok(Ok(())) => eprintln!("stdout-Writer beendet — beende..."),
            }
        }
    }

    // --- Graceful Shutdown ---

    // 1. Alle laufenden FFmpeg-Prozesse via CancellationToken beenden
    global_shutdown_token.cancel();

    // 2. stdin-Reader abbrechen (haelt Arc<JobQueue>, muss weg damit cmd_tx geschlossen wird)
    stdin_abort.abort();

    // 3. Channels schliessen damit Tasks sauber beenden
    //    queue (Arc<JobQueue>) droppen → letzter cmd_tx Ref → run_queue() beendet sich
    drop(queue);
    //    response_tx droppen → write_stdout() beendet sich wenn rx leer
    drop(response_tx);

    // 4. Auf sauberes Beenden der Queue warten (mit Timeout-Fallback)
    let timeout = tokio::time::Duration::from_secs(5);

    if let Err(_) = tokio::time::timeout(timeout, queue_handle).await {
        eprintln!("queue_handle Timeout — wird abgebrochen");
    }

    // stdout-Writer abbrechen (alle Responses sind durch oder Queue ist beendet)
    stdout_abort.abort();

    Ok(())
}
