//! Ingesta async TCP multi-conexión con Tokio (Red C).

use crate::ingress::dispatch::{drain_decoded_frames, iteration_slice};
use crate::ingress::wire::{encode_frame, FRAME_LEN};
use crate::runner::BenchmarkError;
use crate::workload::{PacketFormat, WorkloadConfig};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

/// Parámetros de una ejecución async TCP.
struct AsyncTcpRun {
    bind_port: u16,
    connections: usize,
    read_batch_size: usize,
    iterations: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
    job_tx: Sender<WorkloadConfig>,
}

/// Inicia el productor async TCP: listener local + clientes concurrentes.
///
/// # Inputs
///
/// - `config`: puerto, conexiones y tamaño de batch de lectura.
/// - `iterations`: payloads totales repartidos entre conexiones.
/// - `payload_bytes`, `format`, `seed`: plantilla por iteración.
/// - `job_tx`: canal hacia los workers del benchmark.
///
/// # Returns
///
/// Join handle del hilo que ejecuta el runtime Tokio.
pub fn spawn_async_tcp_producer(
    config: &crate::ingress::IngressConfig,
    iterations: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
    job_tx: Sender<WorkloadConfig>,
) -> JoinHandle<Result<(), BenchmarkError>> {
    let bind_port = config.bind_port;
    let connections = config.connections;
    let read_batch_size = config.read_batch_size;

    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(connections.max(2))
            .thread_name("ingress-async")
            .build()
            .map_err(|err| {
                BenchmarkError::InvalidConfig(format!("tokio runtime init failed: {err}"))
            })?;

        runtime.block_on(run_async_tcp_ingress(AsyncTcpRun {
            bind_port,
            connections,
            read_batch_size,
            iterations,
            payload_bytes,
            format,
            seed,
            job_tx,
        }))
    })
}

async fn run_async_tcp_ingress(run: AsyncTcpRun) -> Result<(), BenchmarkError> {
    let AsyncTcpRun {
        bind_port,
        connections,
        read_batch_size,
        iterations,
        payload_bytes,
        format,
        seed,
        job_tx,
    } = run;

    let bind_addr = format!("127.0.0.1:{bind_port}");
    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|err| BenchmarkError::InvalidConfig(format!("tcp bind failed on {bind_addr}: {err}")))?;

    let job_tx = Arc::new(job_tx);
    let mut client_tasks = JoinSet::new();
    for conn_id in 0..connections {
        let bind_addr = bind_addr.clone();
        client_tasks.spawn(async move {
            let (start, count) = iteration_slice(iterations, connections, conn_id);
            send_client_frames(
                &bind_addr,
                start,
                count,
                payload_bytes,
                format,
                seed,
            )
            .await
        });
    }

    let mut server_tasks = JoinSet::new();
    for _ in 0..connections {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|err| BenchmarkError::InvalidConfig(format!("tcp accept failed: {err}")))?;

        let job_tx = Arc::clone(&job_tx);
        server_tasks.spawn(async move {
            read_connection_frames(stream, read_batch_size, job_tx).await
        });
    }

    while let Some(result) = client_tasks.join_next().await {
        result.map_err(|_| BenchmarkError::WorkerPanicked)??;
    }

    while let Some(result) = server_tasks.join_next().await {
        result.map_err(|_| BenchmarkError::WorkerPanicked)??;
    }

    Ok(())
}

async fn read_connection_frames(
    mut stream: TcpStream,
    read_batch_size: usize,
    job_tx: Arc<Sender<WorkloadConfig>>,
) -> Result<(), BenchmarkError> {
    let chunk = FRAME_LEN * read_batch_size.max(1);
    let mut buffer = Vec::with_capacity(chunk);
    let mut read_buf = vec![0_u8; chunk];

    loop {
        let read = stream
            .read(&mut read_buf)
            .await
            .map_err(|err| BenchmarkError::InvalidConfig(format!("tcp read failed: {err}")))?;

        if read == 0 {
            break;
        }

        buffer.extend_from_slice(&read_buf[..read]);
        drain_decoded_frames(&mut buffer, &job_tx)?;
    }

    if !buffer.is_empty() {
        return Err(BenchmarkError::InvalidConfig(format!(
            "connection closed with partial frame ({} bytes)",
            buffer.len()
        )));
    }

    Ok(())
}

async fn send_client_frames(
    bind_addr: &str,
    start_iteration: u64,
    count: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
) -> Result<(), BenchmarkError> {
    let mut stream = connect_with_retry(bind_addr).await?;

    let mut frame = [0_u8; FRAME_LEN];
    for offset in 0..count {
        let iteration = start_iteration + offset;
        let config = WorkloadConfig::new(payload_bytes, format, seed.wrapping_add(iteration))?;
        encode_frame(&mut frame, &config)?;
        stream
            .write_all(&frame)
            .await
            .map_err(|err| BenchmarkError::InvalidConfig(format!("tcp write failed: {err}")))?;
    }

    stream
        .shutdown()
        .await
        .map_err(|err| BenchmarkError::InvalidConfig(format!("tcp shutdown failed: {err}")))?;
    Ok(())
}

async fn connect_with_retry(bind_addr: &str) -> Result<TcpStream, BenchmarkError> {
    for attempt in 0..50 {
        match TcpStream::connect(bind_addr).await {
            Ok(stream) => return Ok(stream),
            Err(err) if attempt + 1 < 50 => {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                let _ = err;
            }
            Err(err) => {
                return Err(BenchmarkError::InvalidConfig(format!(
                    "tcp connect failed on {bind_addr}: {err}"
                )));
            }
        }
    }

    Err(BenchmarkError::InvalidConfig(format!(
        "tcp connect timed out on {bind_addr}"
    )))
}
