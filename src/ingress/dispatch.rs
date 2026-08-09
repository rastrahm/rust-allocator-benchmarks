//! Reparto de iteraciones y drenado de frames compartido entre modos de red.

#![cfg_attr(
    not(any(feature = "ingress-async", feature = "ingress-io-uring")),
    allow(dead_code)
)]

use crate::ingress::wire::{decode_frame, FRAME_LEN};
use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use crossbeam_channel::Sender;

/// Reparte iteraciones entre conexiones de forma equitativa (+1 al inicio si hay resto).
///
/// # Returns
///
/// Tupla `(start_iteration, count)` para la conexión indicada.
pub fn iteration_slice(total: u64, connections: usize, conn_id: usize) -> (u64, u64) {
    let base = total / connections as u64;
    let remainder = (total % connections as u64) as usize;
    let count = base + if conn_id < remainder { 1 } else { 0 };
    let start = (0..conn_id)
        .map(|id| base + if id < remainder { 1 } else { 0 })
        .sum();
    (start, count)
}

/// Decodifica frames completos desde un buffer acumulado y los encola.
///
/// # Inputs
///
/// - `buffer`: bytes pendientes de una o más conexiones TCP.
/// - `job_tx`: canal hacia los workers del benchmark.
///
/// # Returns
///
/// `Ok(())` tras encolar todos los frames completos, o error de protocolo/canal.
pub fn drain_decoded_frames(
    buffer: &mut Vec<u8>,
    job_tx: &Sender<WorkloadConfig>,
) -> Result<(), BenchmarkError> {
    while buffer.len() >= FRAME_LEN {
        let config = decode_frame(&buffer[..FRAME_LEN])?;
        buffer.drain(..FRAME_LEN);
        job_tx.send(config).map_err(|_| {
            BenchmarkError::InvalidConfig("job channel closed".into())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::wire::encode_frame;
    use crate::workload::PacketFormat;

    #[test]
    fn iteration_slice_covers_all_iterations() {
        let total = 10_u64;
        let connections = 3;
        let mut seen = 0_u64;
        for conn_id in 0..connections {
            let (start, count) = iteration_slice(total, connections, conn_id);
            seen += count;
            assert!(start + count <= total);
        }
        assert_eq!(seen, total);
    }

    #[test]
    fn drain_decoded_frames_enqueues_jobs() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let config = WorkloadConfig::new(128, PacketFormat::JsonLike, 7).expect("valid");
        let mut frame = [0_u8; FRAME_LEN];
        encode_frame(&mut frame, &config).expect("encode");

        let mut buffer = frame.to_vec();
        drain_decoded_frames(&mut buffer, &tx).expect("drain");
        assert!(buffer.is_empty());
        assert_eq!(rx.recv().expect("job"), config);
    }
}
