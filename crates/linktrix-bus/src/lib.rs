//! Interfaces shared by the proprietary Linktrix Tools application and its
//! separately licensed open-source ASIO components (the `Linktrix Audio 64 IO`
//! driver DLL and the `linktrix_asio_host` helper). MIT licensed, so either
//! side may link it. It contains no ASIO SDK material: only Linktrix's own
//! shared-memory layouts and sample packing.
//!
//! - `bus`: the 64-channel DAW bus between the engine and the ASIO driver DLL.
//! - `bridge`: the per-device exchange between the engine and an ASIO helper
//!   process (see docs/ASIO-HELPER-PROTOCOL.md).
//! - `pcm`: 24-bit sample packing and the sample containers both sides use.
#[cfg(windows)]
pub mod bridge;
#[cfg(windows)]
pub mod bus;
pub mod pcm;

/// UTF-16, NUL terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
