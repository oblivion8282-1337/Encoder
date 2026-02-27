// IPC-Server: Liest JSON-Requests von stdin, schreibt Responses auf stdout (NDJSON).

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;

use crate::ipc::protocol::{Request, Response};
use crate::jobs::transcode::{Job, JobQueue};

/// Liest Requests von stdin und gibt sie via Sender weiter.
/// Alle Responses werden ueber `response_tx` gesendet, damit nur ein
/// einziger Writer-Task auf stdout schreibt (keine Race Condition).
pub async fn read_stdin(
    queue: Arc<JobQueue>,
    response_tx: mpsc::Sender<Response>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Ungueltige JSON-Nachricht: {e}");
                continue;
            }
        };

        match request {
            Request::AddJob {
                id,
                input_path,
                output_dir,
                mode,
                options,
            } => {
                let job_id = id.clone();
                let job = Job::new(id, input_path, output_dir, mode, options);
                if let Err(e) = queue.add_job(job).await {
                    eprintln!("Fehler beim Hinzufuegen des Jobs: {e}");
                    let _ = response_tx.send(Response::JobError {
                        id: job_id,
                        message: format!("Job konnte nicht hinzugefuegt werden: {e}"),
                    }).await;
                }
            }
            Request::CancelJob { id } => {
                let cancel_id = id.clone();
                if let Err(e) = queue.cancel_job(id).await {
                    eprintln!("Fehler beim Abbrechen des Jobs: {e}");
                    let _ = response_tx.send(Response::JobError {
                        id: cancel_id,
                        message: format!("Job konnte nicht abgebrochen werden: {e}"),
                    }).await;
                }
            }
            Request::GetStatus => {
                match queue.get_status().await {
                    Ok(statuses) => {
                        let response = Response::StatusReport { jobs: statuses };
                        if let Err(e) = response_tx.send(response).await {
                            eprintln!("Fehler beim Senden der Status-Response: {e}");
                        }
                    }
                    Err(e) => eprintln!("Fehler beim Abfragen des Status: {e}"),
                }
            }
            Request::Shutdown => {
                let _ = shutdown_tx.send(());
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Schreibt Response-Events als NDJSON auf stdout.
/// Laeuft als eigener Task. Gibt Fehler zurueck wenn die stdout-Pipe geschlossen wird.
pub async fn write_stdout(mut rx: mpsc::Receiver<Response>) -> Result<()> {
    let stdout = tokio::io::stdout();
    let mut writer = BufWriter::new(stdout);

    while let Some(response) = rx.recv().await {
        let json = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Fehler beim Serialisieren der Response: {e}");
                continue;
            }
        };

        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}
