//! Shared helpers for the integration-test binaries (not a test target
//! itself: only top-level `tests/*.rs` files become test binaries).

/// True if `bytes` contains any braille pattern char (U+2800..=U+28FF,
/// UTF-8: E2 A0..A3 80..BF).
pub fn has_braille(bytes: &[u8]) -> bool {
    bytes
        .windows(3)
        .any(|w| w[0] == 0xE2 && (0xA0..=0xA3).contains(&w[1]) && (0x80..=0xBF).contains(&w[2]))
}
