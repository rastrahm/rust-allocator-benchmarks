//! Hook sin aceleración — comportamiento original del benchmark.

use crate::accelerator::AcceleratorHook;
use crate::runner::BenchmarkError;
use crate::workload::WorkloadConfig;
use std::time::Duration;

/// Acelerador no-op: no añade latencia ni trabajo extra.
pub struct NoAccelerator;

impl AcceleratorHook for NoAccelerator {
    /// Devuelve latencia cero sin ejecutar trabajo GPU.
    fn post_process(
        &self,
        _job: &WorkloadConfig,
        _parse_latency: Duration,
    ) -> Result<Duration, BenchmarkError> {
        Ok(Duration::ZERO)
    }
}
