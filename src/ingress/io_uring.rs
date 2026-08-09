//! Ingesta production-like con io_uring via tokio-uring (Red D).

use crate::ingress::dispatch::{drain_decoded_frames, iteration_slice};
use crate::ingress::wire::{encode_frame, FRAME_LEN};
use crate::runner::BenchmarkError;
use crate::workload::{PacketFormat, WorkloadConfig};
use crossbeam_channel::Sender;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio_uring::net::{TcpListener, TcpStream};

/// Parámetros de una ejecución io_uring.
struct IoUringRun {
    bind_port: u16,
    connections: usize,
    read_batch_size: usize,
    iterations: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
    job_tx: Sender<WorkloadConfig>,
}

/// Inicia el productor io_uring: listener + clientes sobre tokio-uring.
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
/// Join handle del hilo que ejecuta `tokio_uring::start`.
pub fn spawn_io_uring_producer(
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
        tokio_uring::start(async move {
            run_io_uring_ingress(IoUringRun {
                bind_port,
                connections,
                read_batch_size,
                iterations,
                payload_bytes,
                format,
                seed,
                job_tx,
            })
            .await
        })
    })
}

async fn run_io_uring_ingress(run: IoUringRun) -> Result<(), BenchmarkError> {
    let IoUringRun {
        bind_port,
        connections,
        read_batch_size,
        iterations,
        payload_bytes,
        format,
        seed,
        job_tx,
    } = run;

    let bind_addr: SocketAddr = format!("127.0.0.1:{bind_port}")
        .parse()
        .map_err(|err| BenchmarkError::InvalidConfig(format!("invalid bind address: {err}")))?;

    let listener = TcpListener::bind(bind_addr).map_err(|err| {
        BenchmarkError::InvalidConfig(format!("io-uring tcp bind failed on {bind_addr}: {err}"))
    })?;

    let job_tx = Arc::new(job_tx);
    let mut client_handles = Vec::with_capacity(connections);
    for conn_id in 0..connections {
        let (start, count) = iteration_slice(iterations, connections, conn_id);
        client_handles.push(tokio_uring::spawn(send_client_frames(
            bind_addr,
            start,
            count,
            payload_bytes,
            format,
            seed,
        )));
    }

    let mut server_handles = Vec::with_capacity(connections);
    for _ in 0..connections {
        let (stream, _) = listener.accept().await.map_err(|err| {
            BenchmarkError::InvalidConfig(format!("io-uring tcp accept failed: {err}"))
        })?;
        let job_tx = Arc::clone(&job_tx);
        server_handles.push(tokio_uring::spawn(read_connection_frames(
            stream,
            read_batch_size,
            job_tx,
        )));
    }

    for handle in client_handles {
        handle
            .await
            .map_err(|_| BenchmarkError::WorkerPanicked)??;
    }

    for handle in server_handles {
        handle
            .await
            .map_err(|_| BenchmarkError::WorkerPanicked)??;
    }

    Ok(())
}

async fn send_client_frames(
    bind_addr: SocketAddr,
    start_iteration: u64,
    count: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
) -> Result<(), BenchmarkError> {
    let stream = connect_with_retry(bind_addr).await?;
    let mut frame = [0_u8; FRAME_LEN];

    for offset in 0..count {
        let iteration = start_iteration + offset;
        let config = WorkloadConfig::new(payload_bytes, format, seed.wrapping_add(iteration))?;
        encode_frame(&mut frame, &config)?;
        let (result, _) = stream.write_all(frame.to_vec()).await;
        result.map_err(|err| BenchmarkError::InvalidConfig(format!("io-uring tcp write failed: {err}")))?;
    }

    Ok(())
}

async fn connect_with_retry(bind_addr: SocketAddr) -> Result<TcpStream, BenchmarkError> {
    for attempt in 0..50 {
        match TcpStream::connect(bind_addr).await {
            Ok(stream) => return Ok(stream),
            Err(err) if attempt + 1 < 50 => {
                tokio::time::sleep(Duration::from_millis(2)).await;
                let _ = err;
            }
            Err(err) => {
                return Err(BenchmarkError::InvalidConfig(format!(
                    "io-uring tcp connect failed on {bind_addr}: {err}"
                )));
            }
        }
    }

    Err(BenchmarkError::InvalidConfig(format!(
        "io-uring tcp connect timed out on {bind_addr}"
    )))
}

async fn read_connection_frames(
    stream: TcpStream,
    read_batch_size: usize,
    job_tx: Arc<Sender<WorkloadConfig>>,
) -> Result<(), BenchmarkError> {
    let chunk = FRAME_LEN * read_batch_size.max(1);
    let mut buffer = Vec::with_capacity(chunk);
    let mut read_buf = vec![0_u8; chunk];

    loop {
        let (result, returned_buf) = stream.read(read_buf).await;
        read_buf = returned_buf;
        let read = result
            .map_err(|err| BenchmarkError::InvalidConfig(format!("io-uring tcp read failed: {err}")))?;

        if read == 0 {
            break;
        }

        buffer.extend_from_slice(&read_buf[..read]);
        drain_decoded_frames(&mut buffer, &job_tx)?;
    }

    if !buffer.is_empty() {
        return Err(BenchmarkError::InvalidConfig(format!(
            "io-uring connection closed with partial frame ({} bytes)",
            buffer.len()
        )));
    }

    Ok(())
}
