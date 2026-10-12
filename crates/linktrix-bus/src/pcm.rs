//! Internal sample: signed 24-bit integer, sign-extended to i32.
//!
//! Container codes (`kind`): 17 packed 24-bit LE, 18 32-bit LE holding 24
//! bits in the top bytes, 27 32-bit LE holding 24 bits in the low bytes,
//! 19 32-bit float, 20 64-bit float. They match the numbers audio drivers
//! report for these formats, so a driver's value can be passed through.
pub fn unpack24(b: &[u8]) -> i32 {
    ((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8 >> 8
}
pub fn pack24(x: i32, b: &mut [u8]) {
    let v = x.clamp(-8_388_608, 8_388_607).to_le_bytes();
    b[..3].copy_from_slice(&v[..3]);
}
pub fn sample_bytes(kind: i32) -> Option<usize> {
    match kind {
        17 => Some(3),
        18 | 19 | 27 => Some(4),
        20 => Some(8),
        _ => None,
    }
}
pub fn decode(kind: i32, b: &[u8]) -> i32 {
    match kind {
        17 => unpack24(b),
        18 => i32::from_le_bytes(b[..4].try_into().unwrap()) >> 8,
        27 => i32::from_le_bytes(b[..4].try_into().unwrap()).clamp(-8_388_608, 8_388_607),
        19 => quantize(f32::from_le_bytes(b[..4].try_into().unwrap()) as f64),
        20 => quantize(f64::from_le_bytes(b[..8].try_into().unwrap())),
        _ => 0,
    }
}
fn quantize(v: f64) -> i32 {
    if v.is_finite() {
        (v * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32
    } else {
        0
    }
}
pub fn encode(kind: i32, x: i32, b: &mut [u8]) {
    let x = x.clamp(-8_388_608, 8_388_607);
    match kind {
        17 => pack24(x, b),
        18 => b[..4].copy_from_slice(&(x << 8).to_le_bytes()),
        27 => b[..4].copy_from_slice(&x.to_le_bytes()),
        19 => b[..4].copy_from_slice(&(x as f32 / 8_388_608.0).to_le_bytes()),
        20 => b[..8].copy_from_slice(&(x as f64 / 8_388_608.0).to_le_bytes()),
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample_roundtrip() {
        for kind in [17, 18, 19, 20, 27] {
            for x in [-8_388_608, -123456, -1, 0, 1, 123456, 8_388_607] {
                let mut b = [0; 8];
                encode(kind, x, &mut b);
                assert_eq!(decode(kind, &b), x, "kind {kind}");
            }
        }
    }
    #[test]
    fn saturates() {
        let mut b = [0; 3];
        pack24(i32::MAX, &mut b);
        assert_eq!(unpack24(&b), 8_388_607);
    }
}
