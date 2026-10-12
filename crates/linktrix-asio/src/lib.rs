//! The Windows x64 IASIO interface, translated from the Steinberg ASIO SDK,
//! for the Linktrix open-source ASIO components: the `Linktrix Audio 64 IO`
//! driver DLL (`wat-asio`) and the hardware helper (`linktrix-asio-host`).
//!
//! Copyright (c) 2026 Linktrix. Licensed under the GNU General Public License,
//! version 3 only (see COPYING). The ASIO SDK is used under its GPLv3 option;
//! its original notices are in third_party/asio. ASIO is a trademark and
//! software of Steinberg Media Technologies GmbH.
#![cfg(windows)]
pub mod abi;
pub use abi::*;

/// Whether a driver accepts `n` frames per buffer, given its reported
/// minimum, maximum and granularity (-1: powers of two; 0: only `min`).
pub fn buffer_allowed(n: i32, min: i32, max: i32, gran: i32) -> bool {
    (min..=max).contains(&n)
        && (16..=2048).contains(&n)
        && match gran {
            -1 => (n as u32).is_power_of_two(),
            0 => min == max && n == min,
            g if g > 0 => (n - min) % g == 0,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_vendor_buffer_rules() {
        assert!(buffer_allowed(128, 32, 1024, -1));
        assert!(!buffer_allowed(96, 32, 1024, -1));
        assert!(buffer_allowed(96, 32, 256, 32));
        assert!(!buffer_allowed(100, 32, 256, 32));
        assert!(buffer_allowed(128, 128, 128, 0));
        assert!(!buffer_allowed(4096, 32, 8192, -1));
    }
    #[test]
    fn wire_abi() {
        assert_eq!(std::mem::size_of::<BufferInfo>(), 24);
        assert_eq!(std::mem::size_of::<ChannelInfo>(), 52);
        assert_eq!(std::mem::size_of::<Callbacks>(), 32);
        assert_eq!(std::mem::size_of::<Samples>(), 8);
    }
}
