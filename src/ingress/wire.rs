//! Protocolo binario fijo para transportar [`WorkloadConfig`] sobre TCP.

#![cfg_attr(not(feature = "ingress-async"), allow(dead_code))]

use crate::runner::BenchmarkError;
use crate::workload::{PacketFormat, WorkloadConfig};

/// Longitud fija de un frame de ingesta en la red simulada (magic + payload + format + seed).
pub const FRAME_LEN: usize = 15;

const MAGIC: u16 = 0xAB17;

/// Codifica un frame de workload en un buffer de exactamente [`FRAME_LEN`] bytes.
///
/// # Inputs
///
/// - `out`: destino con al menos [`FRAME_LEN`] bytes.
/// - `config`: configuración a serializar.
///
/// # Returns
///
/// `Ok(())` o error si el buffer es demasiado corto.
pub fn encode_frame(out: &mut [u8], config: &WorkloadConfig) -> Result<(), BenchmarkError> {
    if out.len() < FRAME_LEN {
        return Err(BenchmarkError::InvalidConfig(
            "wire frame buffer too small".into(),
        ));
    }

    out[0..2].copy_from_slice(&MAGIC.to_le_bytes());
    out[2..6].copy_from_slice(&(config.payload_bytes as u32).to_le_bytes());
    out[6] = format_to_byte(config.format);
    out[7..15].copy_from_slice(&config.seed.to_le_bytes());

    Ok(())
}

/// Decodifica un frame de workload desde un buffer de al menos [`FRAME_LEN`] bytes.
///
/// # Returns
///
/// Configuración validada lista para procesar, o error de protocolo/workload.
pub fn decode_frame(bytes: &[u8]) -> Result<WorkloadConfig, BenchmarkError> {
    if bytes.len() < FRAME_LEN {
        return Err(BenchmarkError::InvalidConfig(
            "incomplete ingress wire frame".into(),
        ));
    }

    let magic = u16::from_le_bytes([bytes[0], bytes[1]]);
    if magic != MAGIC {
        return Err(BenchmarkError::InvalidConfig(format!(
            "invalid ingress wire magic: {magic:#06x}"
        )));
    }

    let payload_bytes = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize;
    let format = byte_to_format(bytes[6])?;
    let seed = u64::from_le_bytes([
        bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
    ]);

    WorkloadConfig::new(payload_bytes, format, seed).map_err(BenchmarkError::from)
}

fn format_to_byte(format: PacketFormat) -> u8 {
    match format {
        PacketFormat::JsonLike => 0,
        PacketFormat::BorshLike => 1,
    }
}

fn byte_to_format(byte: u8) -> Result<PacketFormat, BenchmarkError> {
    match byte {
        0 => Ok(PacketFormat::JsonLike),
        1 => Ok(PacketFormat::BorshLike),
        other => Err(BenchmarkError::InvalidConfig(format!(
            "unknown packet format byte: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_json_frame() {
        let config = WorkloadConfig::new(512, PacketFormat::JsonLike, 42).expect("valid");
        let mut buf = [0_u8; FRAME_LEN];
        encode_frame(&mut buf, &config).expect("encode");
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded, config);
    }

    #[test]
    fn roundtrip_borsh_frame() {
        let config = WorkloadConfig::new(256, PacketFormat::BorshLike, 99).expect("valid");
        let mut buf = [0_u8; FRAME_LEN];
        encode_frame(&mut buf, &config).expect("encode");
        let decoded = decode_frame(&buf).expect("decode");
        assert_eq!(decoded, config);
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut buf = [0_u8; FRAME_LEN];
        let config = WorkloadConfig::new(256, PacketFormat::JsonLike, 0).expect("valid");
        encode_frame(&mut buf, &config).expect("encode");
        buf[0] = 0xFF;
        let err = decode_frame(&buf).unwrap_err();
        assert!(matches!(err, BenchmarkError::InvalidConfig(_)));
    }
}
