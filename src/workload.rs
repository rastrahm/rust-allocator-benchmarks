//! Simulación de ingesta de payloads con patrones intensivos de alloc/dealloc.
//!
//! Reproduce la presión de heap de parsear paquetes de red estilo JSON o Borsh:
//! buffers crudos, strings, vectores anidados y liberación inmediata al terminar.

use crate::metrics::{LatencyRecorder, MetricsError};
use hdrhistogram::sync::Recorder;
use std::hint::black_box;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Tamaño mínimo de payload (bytes).
const MIN_PAYLOAD_BYTES: usize = 64;

/// Tamaño máximo de payload (bytes): 4 MiB.
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Formato simulado del paquete de red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum PacketFormat {
    /// Patrón con strings, mapas lógicos y estructuras anidadas (similar a JSON).
    #[default]
    JsonLike,
    /// Patrón binario compacto con buffers length-prefixed (similar a Borsh).
    BorshLike,
}

/// Configuración del simulador de carga de red.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadConfig {
    /// Tamaño del buffer crudo simulado recibido del socket.
    pub payload_bytes: usize,
    /// Formato de parsing a simular.
    pub format: PacketFormat,
    /// Semilla determinista para el contenido del payload.
    pub seed: u64,
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        Self {
            payload_bytes: 2_048,
            format: PacketFormat::JsonLike,
            seed: 0,
        }
    }
}

impl WorkloadConfig {
    /// Crea una configuración validada.
    ///
    /// # Inputs
    ///
    /// - `payload_bytes`: tamaño del buffer simulado, entre 64 B y 4 MiB.
    /// - `format`: estilo de parsing (`JsonLike` o `BorshLike`).
    /// - `seed`: semilla para contenido reproducible.
    ///
    /// # Returns
    ///
    /// Configuración lista para [`process_payload`], o [`WorkloadError::InvalidPayloadSize`].
    pub fn new(payload_bytes: usize, format: PacketFormat, seed: u64) -> Result<Self, WorkloadError> {
        if !(MIN_PAYLOAD_BYTES..=MAX_PAYLOAD_BYTES).contains(&payload_bytes) {
            return Err(WorkloadError::InvalidPayloadSize {
                size: payload_bytes,
                min: MIN_PAYLOAD_BYTES,
                max: MAX_PAYLOAD_BYTES,
            });
        }

        Ok(Self {
            payload_bytes,
            format,
            seed,
        })
    }
}

/// Errores del simulador de workload.
#[derive(Debug, Error)]
pub enum WorkloadError {
    /// Tamaño de payload fuera del rango permitido.
    #[error("payload size {size} is out of range [{min}, {max}] bytes")]
    InvalidPayloadSize {
        size: usize,
        min: usize,
        max: usize,
    },

    /// Error al registrar la latencia en métricas.
    #[error(transparent)]
    Metrics(#[from] MetricsError),
}

/// Procesa un payload simulado y devuelve la latencia de punta a punta.
///
/// # Inputs
///
/// - `config`: tamaño, formato y semilla del payload.
///
/// # Returns
///
/// Duración desde la recepción del buffer hasta la liberación de las estructuras parseadas.
pub fn process_payload(config: &WorkloadConfig) -> Duration {
    let started = Instant::now();
    let raw = generate_payload(config.payload_bytes, config.seed);

    let checksum = match config.format {
        PacketFormat::JsonLike => black_box(parse_json_like(&raw).checksum()),
        PacketFormat::BorshLike => black_box(parse_borsh_like(&raw).checksum()),
    };

    black_box(checksum);
    started.elapsed()
}

/// Ejecuta una iteración del workload y registra su latencia.
///
/// # Inputs
///
/// - `config`: configuración del payload simulado.
/// - `writer`: handle de métricas del hilo actual.
///
/// # Returns
///
/// La latencia medida, o un error de registro en métricas.
pub fn process_and_record(
    config: &WorkloadConfig,
    writer: &mut Recorder<u64>,
) -> Result<Duration, WorkloadError> {
    let latency = process_payload(config);
    LatencyRecorder::record_duration(writer, latency)?;
    Ok(latency)
}

/// Genera bytes pseudo-aleatorios deterministas simulando datos de red.
///
/// # Inputs
///
/// - `size`: cantidad de bytes a generar.
/// - `seed`: semilla inicial del generador xorshift.
///
/// # Returns
///
/// Buffer con contenido reproducible para el par `(size, seed)`.
fn generate_payload(size: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.max(1);
    let mut payload = Vec::with_capacity(size);

    for _ in 0..size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        payload.push(state as u8);
    }

    payload
}

/// Instrucción anidada en un paquete estilo JSON.
#[derive(Debug)]
struct JsonInstruction {
    program_id: String,
    accounts: Vec<String>,
    data: Vec<u8>,
}

/// Paquete parseado con estructura similar a JSON de transacciones.
#[derive(Debug)]
struct JsonPacket {
    version: String,
    signatures: Vec<String>,
    instructions: Vec<JsonInstruction>,
    metadata: Vec<u8>,
}

impl JsonPacket {
    fn checksum(&self) -> u64 {
        let mut sum = self.version.len() as u64;
        sum = sum.wrapping_add(self.metadata.len() as u64);
        sum = sum.wrapping_add(self.signatures.len() as u64);
        sum = sum.wrapping_add(self.instructions.len() as u64);
        for instruction in &self.instructions {
            sum = sum.wrapping_add(instruction.data.len() as u64);
            sum = sum.wrapping_add(instruction.program_id.len() as u64);
            sum = sum.wrapping_add(instruction.accounts.len() as u64);
        }
        sum
    }
}

/// Simula parsing JSON: strings, vectores anidados y copias del buffer original.
fn parse_json_like(raw: &[u8]) -> JsonPacket {
    let len = raw.len().max(1);
    let version = String::from_utf8_lossy(&raw[..len.min(16)]).into_owned();

    let signatures: Vec<String> = (0..4)
        .map(|index| {
            let start = (index * 17) % len;
            let end = (start + 32).min(len);
            format!("sig_{index}_{}", hex_preview(&raw[start..end]))
        })
        .collect();

    let instructions: Vec<JsonInstruction> = (0..8)
        .map(|index| {
            let chunk_size = 24 + (index % 40);
            let start = (index * 53) % len;
            let data: Vec<u8> = raw
                .iter()
                .cycle()
                .skip(start)
                .take(chunk_size)
                .copied()
                .collect();

            JsonInstruction {
                program_id: format!("program_{index}"),
                accounts: (0..3)
                    .map(|account| format!("account_{index}_{account}"))
                    .collect(),
                data,
            }
        })
        .collect();

    JsonPacket {
        version,
        signatures,
        instructions,
        metadata: raw.to_vec(),
    }
}

/// Paquete parseado con layout binario compacto estilo Borsh.
#[derive(Debug)]
struct BorshPacket {
    header: Vec<u8>,
    accounts: Vec<Vec<u8>>,
    instruction_data: Vec<Vec<u8>>,
}

impl BorshPacket {
    fn checksum(&self) -> u64 {
        let mut sum = self.header.len() as u64;
        for account in &self.accounts {
            sum = sum.wrapping_add(account.len() as u64);
        }
        for data in &self.instruction_data {
            sum = sum.wrapping_add(data.len() as u64);
        }
        sum
    }
}

/// Simula parsing Borsh: headers fijos, campos length-prefixed y menos strings.
fn parse_borsh_like(raw: &[u8]) -> BorshPacket {
    let len = raw.len().max(1);
    let header = raw[..len.min(32)].to_vec();

    let account_count = (raw.first().copied().unwrap_or(1) as usize % 12) + 1;
    let accounts: Vec<Vec<u8>> = (0..account_count)
        .map(|index| {
            let size = 32 + (index % 96);
            raw.iter().cycle().skip(index * 7).take(size).copied().collect()
        })
        .collect();

    let instruction_count = (raw.get(1).copied().unwrap_or(2) as usize % 8) + 2;
    let instruction_data: Vec<Vec<u8>> = (0..instruction_count)
        .map(|index| {
            let size = 16 + (index % 128);
            raw.iter()
                .cycle()
                .skip(index * 11)
                .take(size)
                .copied()
                .collect()
        })
        .collect();

    BorshPacket {
        header,
        accounts,
        instruction_data,
    }
}

/// Produce una vista hexadecimal corta de un slice (para simular IDs en JSON).
fn hex_preview(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::LatencyRecorder;

    #[test]
    fn rejects_payload_size_below_minimum() {
        let err = WorkloadConfig::new(32, PacketFormat::JsonLike, 0).unwrap_err();
        assert!(matches!(
            err,
            WorkloadError::InvalidPayloadSize {
                size: 32,
                min: MIN_PAYLOAD_BYTES,
                max: MAX_PAYLOAD_BYTES,
            }
        ));
    }

    #[test]
    fn rejects_payload_size_above_maximum() {
        let err = WorkloadConfig::new(MAX_PAYLOAD_BYTES + 1, PacketFormat::BorshLike, 0)
            .unwrap_err();
        assert!(matches!(
            err,
            WorkloadError::InvalidPayloadSize {
                size,
                min: MIN_PAYLOAD_BYTES,
                max: MAX_PAYLOAD_BYTES,
            } if size == MAX_PAYLOAD_BYTES + 1
        ));
    }

    #[test]
    fn generate_payload_is_deterministic_for_seed() {
        let a = generate_payload(256, 42);
        let b = generate_payload(256, 42);
        let c = generate_payload(256, 43);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn process_payload_completes_for_json_and_borsh_formats() {
        for format in [PacketFormat::JsonLike, PacketFormat::BorshLike] {
            let config = WorkloadConfig::new(512, format, 7).expect("valid config");
            let latency = process_payload(&config);
            assert!(latency > Duration::ZERO);
        }
    }

    #[test]
    fn json_packet_checksum_reflects_nested_allocations() {
        let raw = generate_payload(1_024, 99);
        let packet = parse_json_like(&raw);
        assert_eq!(packet.signatures.len(), 4);
        assert_eq!(packet.instructions.len(), 8);
        assert!(packet.checksum() > 0);
    }

    #[test]
    fn borsh_packet_checksum_reflects_binary_layout() {
        let raw = generate_payload(1_024, 11);
        let packet = parse_borsh_like(&raw);
        assert!(!packet.header.is_empty());
        assert!(!packet.accounts.is_empty());
        assert!(packet.checksum() > 0);
    }

    #[test]
    fn integrates_with_latency_recorder() {
        let config = WorkloadConfig::new(768, PacketFormat::JsonLike, 5).expect("valid config");
        let recorder = LatencyRecorder::new().expect("recorder should initialize");
        let mut writer = recorder.recorder();

        for _ in 0..50 {
            process_and_record(&config, &mut writer).expect("record should succeed");
        }
        drop(writer);

        let snap = recorder.snapshot();
        assert_eq!(snap.count, 50);
        assert!(snap.max_ns >= snap.min_ns);
    }
}
