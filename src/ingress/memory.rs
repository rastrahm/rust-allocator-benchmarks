//! Productor in-memory via canal crossbeam (comportamiento original).

use crate::runner::BenchmarkError;
use crate::workload::{PacketFormat, WorkloadConfig};
use crossbeam_channel::Sender;
use std::thread::{self, JoinHandle};

/// Encola configs de workload en un canal acotado (modo in-memory).
///
/// # Inputs
///
/// - `job_tx`: emisor del canal compartido con los workers.
/// - `iterations`: cantidad total de payloads.
/// - `payload_bytes`, `format`, `seed`: plantilla por iteración.
///
/// # Returns
///
/// Join handle del hilo productor.
pub fn spawn_memory_producer(
    job_tx: Sender<WorkloadConfig>,
    iterations: u64,
    payload_bytes: usize,
    format: PacketFormat,
    seed: u64,
) -> JoinHandle<Result<(), BenchmarkError>> {
    thread::spawn(move || {
        for iteration in 0..iterations {
            let workload =
                WorkloadConfig::new(payload_bytes, format, seed.wrapping_add(iteration))?;
            job_tx.send(workload).map_err(|_| {
                BenchmarkError::InvalidConfig("job channel closed".into())
            })?;
        }
        Ok(())
    })
}
