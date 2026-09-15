//! Land polygon decoder for the embedded Natural Earth 110m coastline data
//! (see PLAN.md §7). This module's docs below are the wire-format
//! specification; the encoder lives in `examples/datagen.rs`.
//!
//! # Wire format
//!
//! [`LAND_DATA`](crate::coast_data::LAND_DATA) is a single base85 string. Decoding
//! layers, outside-in:
//!
//! **1. base85 → bytes.** The byte stream is taken in 4-byte groups. Each group is read
//! as a big-endian `u32` and emitted as 5 base-85 digits, most significant digit first,
//! using the custom 85-character ASCII alphabet
//! `0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#`
//! (the Z85 alphabet — chosen because it contains no quote, backslash, or newline, so
//! the encoded text embeds verbatim inside a Rust string literal). A trailing partial
//! group of `n` bytes (1..=3) is zero-padded on the right to a full 4 bytes and only its
//! first `n + 1` digits are emitted; therefore a valid encoding is always `5*k` or
//! `5*k + (2..=4)` characters long. Decoding is the exact inverse: every complete group
//! of 5 characters yields 4 bytes. A final short group of `k` characters is completed to
//! 5 digits with the *maximum* digit (84) — not zero, because the encoder pads with zero
//! *bytes*, and the dropped low base-85 digits of such a value are generally non-zero.
//! The completed value `V'` then satisfies `V <= V' < V + 85^(5-k) < V + 256^(5-k)`, so
//! the group's top `k - 1` bytes are exactly the original `n` bytes. A dangling single
//! trailing character is malformed and rejected.
//!
//! **2. varints.** Unsigned integers use LEB128-style varints: 7 payload bits per byte,
//! least-significant 7-bit group first, high bit set = "more bytes follow".
//!
//! **3. coordinates.** Angles are quantized to *centidegrees*: `c = round(deg * 100)` as
//! `i64` (0.01° ≈ 1.1 km — below the 110m source resolution). Per ring, each coordinate
//! is delta-coded against the previous vertex of the *same* ring (the first vertex is
//! relative to 0), and every signed delta is zigzag-mapped to unsigned with
//! `(n << 1) ^ (n >> 63)` before varint encoding.
//!
//! **Stream layout (before base85):**
//!
//! ```text
//! varint ring_count
//! per ring, ring_count times:
//!     varint vertex_count
//!     per vertex, vertex_count times:
//!         varint zigzag(d_lat_centideg)   // latitude delta first
//!         varint zigzag(d_lon_centideg)   // then longitude
//! ```
//!
//! Decoding reverses all of this: base85 → bytes → varints → zigzag deltas → absolute
//! centidegrees → `/ 100.0` degrees. Rings come back in source order, keep their
//! GeoJSON closure (the repeated final vertex), and are interpreted with even-odd
//! semantics by the rasterizer. The input is a compile-time constant produced by our
//! own `datagen`, so malformed input panics with a clear message rather than being
//! tolerated silently.

/// One closed ring of (lat_deg, lon_deg) vertices.
pub type Ring = Vec<(f64, f64)>;

/// Z85 alphabet: 85 printable ASCII chars, no quote/backslash/newline.
const ALPHABET: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";

/// Decode a base85/varint/zigzag stream (see module docs) into land rings.
pub fn decode(data: &str) -> Vec<Ring> {
    let bytes = base85_decode(data);
    let mut pos = 0usize;
    let ring_count = read_varint(&bytes, &mut pos);
    // Each ring costs at least one byte (its vertex-count varint), so the
    // remaining stream length is a tight cap against a corrupt header.
    let mut rings = Vec::with_capacity(ring_count.min((bytes.len() - pos) as u64) as usize);
    for _ in 0..ring_count {
        let vertex_count = read_varint(&bytes, &mut pos);
        // Each vertex costs at least two bytes (two varints).
        let remaining = (bytes.len() - pos) as u64;
        let mut ring = Vec::with_capacity(vertex_count.min(remaining / 2) as usize);
        let (mut lat_c, mut lon_c): (i64, i64) = (0, 0);
        for _ in 0..vertex_count {
            lat_c += zigzag_decode(read_varint(&bytes, &mut pos));
            lon_c += zigzag_decode(read_varint(&bytes, &mut pos));
            ring.push((lat_c as f64 / 100.0, lon_c as f64 / 100.0));
        }
        rings.push(ring);
    }
    assert_eq!(pos, bytes.len(), "coast stream has trailing bytes");
    rings
}

/// base85 → bytes, as documented in the module docs (inverse of datagen's encoder).
fn base85_decode(data: &str) -> Vec<u8> {
    let mut digit = [255u8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        digit[c as usize] = i as u8;
    }
    let chars = data.as_bytes();
    assert!(
        chars.len() % 5 != 1,
        "coast base85 stream ends with a dangling digit"
    );
    let mut out = Vec::with_capacity(chars.len() / 5 * 4 + 3);
    let mut i = 0usize;
    while i < chars.len() {
        let k = (chars.len() - i).min(5); // digits in this group (last may be short)
        let mut value: u64 = 0;
        for &c in &chars[i..i + k] {
            let d = digit[c as usize];
            assert!(
                d != 255,
                "coast base85 stream contains invalid char {:#04x}",
                c
            );
            value = value * 85 + d as u64;
        }
        // Complete missing low digits with the maximum digit (84), not zero: the
        // encoder pads with zero *bytes*, so the dropped low base-85 digits are
        // generally non-zero. 84-completion yields V' with V <= V' < V + 85^(5-k)
        // < V + 256^(5-k), so the top k-1 bytes are exactly the original bytes.
        for _ in k..5 {
            value = value * 85 + 84;
        }
        let be = u32::try_from(value)
            .expect("coast base85 group overflows 32 bits")
            .to_be_bytes();
        let keep = k - 1; // full group (k=5) keeps 4 bytes; short group keeps k-1
        out.extend_from_slice(&be[..keep]);
        i += k;
    }
    out
}

/// Read one little-endian 7-bit-group varint starting at `*pos`, advancing it.
/// A 10th byte may only carry a single payload bit (shift 63), so higher bits
/// would be silently shifted out — rejected instead of truncated.
fn read_varint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut value: u64 = 0;
    for shift in (0..64).step_by(7) {
        let b = bytes
            .get(*pos)
            .copied()
            .expect("truncated coast varint stream");
        *pos += 1;
        if shift == 63 {
            assert!(
                b & 0x7f <= 1,
                "coast varint payload overflows 64 bits (byte {b:#04x} at shift 63)"
            );
        }
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return value;
        }
    }
    panic!("coast varint exceeds 64 bits");
}

/// Inverse of the encoder's `(n << 1) ^ (n >> 63)` zigzag map.
fn zigzag_decode(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coast_data::LAND_DATA;

    // ---------------------------------------------------------------------
    // Test-side encoder (inverse of `decode`; mirrors examples/datagen.rs).
    // ---------------------------------------------------------------------

    const ENC_ALPHABET: &[u8; 85] =
        b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";

    fn enc_varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn enc_zigzag(out: &mut Vec<u8>, d: i64) {
        enc_varint(out, ((d << 1) ^ (d >> 63)) as u64);
    }

    fn enc_base85(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(4) * 5);
        for chunk in bytes.chunks(4) {
            let mut value: u32 = 0;
            for &b in chunk {
                value = (value << 8) | b as u32; // big-endian
            }
            value <<= 8 * (4 - chunk.len() as u32); // left-align a partial tail group
            let mut digits = [0u8; 5];
            let mut v = value;
            for d in digits.iter_mut().rev() {
                *d = ENC_ALPHABET[(v % 85) as usize];
                v /= 85;
            }
            let n = chunk.len();
            out.push_str(std::str::from_utf8(&digits[..n + 1]).unwrap());
        }
        out
    }

    /// Encode rings exactly the way datagen does (see module docs).
    fn encode_rings(rings: &[Ring]) -> String {
        let mut out = Vec::new();
        enc_varint(&mut out, rings.len() as u64);
        for ring in rings {
            enc_varint(&mut out, ring.len() as u64);
            let (mut lat_c, mut lon_c): (i64, i64) = (0, 0);
            for &(lat, lon) in ring {
                let l = (lat * 100.0).round() as i64;
                let o = (lon * 100.0).round() as i64;
                enc_zigzag(&mut out, l - lat_c);
                enc_zigzag(&mut out, o - lon_c);
                lat_c = l;
                lon_c = o;
            }
        }
        enc_base85(&out)
    }

    /// Quantize to the format's 0.01° grid, as the encoder does.
    fn q(deg: f64) -> f64 {
        (deg * 100.0).round() / 100.0
    }

    // ---------------------------------------------------------------------
    // Hand-constructed literals (independent of the helper above).
    // ---------------------------------------------------------------------

    #[test]
    fn hand_constructed_literals_decode() {
        // ring_count = 0: bytes [0x00] -> one padded group -> "00".
        assert_eq!(decode("00"), Vec::<Ring>::new());

        // ring_count = 1, vertex_count = 1, both deltas 0: bytes [01,01,00,00] =
        // exactly one full group, value 0x01010000 -> digits (0,27,36,15,2) = "0rAf2".
        assert_eq!(decode("0rAf2"), vec![vec![(0.0, 0.0)]]);
    }

    #[test]
    fn encoder_round_trips() {
        let rings: Vec<Ring> = vec![
            // Small ring with fractional values and both signs.
            vec![(0.0, 0.0), (10.25, -5.5), (20.0, 30.125), (0.0, 0.0)],
            // Closed ring with negative coords and a repeat vertex.
            vec![
                (-33.87, 151.21),
                (-35.0, 150.0),
                (-32.0, 149.5),
                (-33.87, 151.21),
            ],
            // Coordinate extremes exercise the widest deltas (multi-byte varints).
            vec![(-90.0, -180.0), (90.0, 180.0)],
            // A single-vertex ring is representable.
            vec![(41.9, 12.5)],
        ];
        let got = decode(&encode_rings(&rings));
        assert_eq!(got.len(), rings.len());
        for (g, want) in got.iter().zip(&rings) {
            assert_eq!(g.len(), want.len());
            for (v, w) in g.iter().zip(want) {
                assert!((v.0 - q(w.0)).abs() < 1e-9, "lat {v:?} vs {}", q(w.0));
                assert!((v.1 - q(w.1)).abs() < 1e-9, "lon {v:?} vs {}", q(w.1));
            }
        }
    }

    #[test]
    fn base85_round_trips_various_lengths() {
        // Covers full groups and every partial-group tail size (1..3 bytes).
        for len in 0..=13usize {
            let bytes: Vec<u8> = (0..len)
                .map(|i| (i as u8).wrapping_mul(37).wrapping_add(101))
                .collect();
            let text = enc_base85(&bytes);
            assert_eq!(base85_decode(&text), bytes, "len {len}");
        }
    }

    #[test]
    #[should_panic(expected = "dangling digit")]
    fn base85_rejects_dangling_digit() {
        base85_decode("0"); // a lone trailing char encodes zero bytes — malformed
    }

    // ---------------------------------------------------------------------
    // The real generated data.
    // ---------------------------------------------------------------------

    #[test]
    fn land_data_size_budget() {
        // PLAN.md §7 budget: revisit quantization if the constant exceeds 40 KB.
        assert!(
            LAND_DATA.len() <= 40 * 1024,
            "LAND_DATA is {} bytes, over the 40 KiB budget",
            LAND_DATA.len()
        );
    }

    #[test]
    fn land_data_decodes_within_bounds() {
        let rings = decode(LAND_DATA);
        assert!(rings.len() > 100, "only {} rings decoded", rings.len());
        for ring in &rings {
            for &(lat, lon) in ring {
                assert!((-90.0 - 1e-6..=90.0 + 1e-6).contains(&lat), "lat {lat}");
                assert!((-180.0 - 1e-6..=180.0 + 1e-6).contains(&lon), "lon {lon}");
            }
        }
    }

    #[test]
    fn land_data_vertex_counts_match_header() {
        // Walk the raw stream independently and compare the per-ring vertex
        // counts with what decode() produced; the stream must be fully consumed.
        let bytes = base85_decode(LAND_DATA);
        let mut pos = 0usize;
        let ring_count = read_varint(&bytes, &mut pos) as usize;
        let mut counts = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            let n = read_varint(&bytes, &mut pos) as usize;
            assert!(n >= 3, "degenerate ring with {n} vertices");
            counts.push(n);
            for _ in 0..n {
                read_varint(&bytes, &mut pos); // d_lat
                read_varint(&bytes, &mut pos); // d_lon
            }
        }
        assert_eq!(pos, bytes.len(), "stream desynced from header");

        let rings = decode(LAND_DATA);
        let lens: Vec<usize> = rings.iter().map(|r| r.len()).collect();
        assert_eq!(lens, counts);
    }

    #[test]
    fn land_data_contains_rome_probe() {
        // Rome (41.9 N, 12.5 E) must fall inside some ring's bounding box.
        let (rlat, rlon) = (41.9, 12.5);
        let rings = decode(LAND_DATA);
        let hit = rings.iter().any(|ring| {
            let mut min_lat = f64::MAX;
            let mut max_lat = f64::MIN;
            let mut min_lon = f64::MAX;
            let mut max_lon = f64::MIN;
            for &(lat, lon) in ring {
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
            }
            (min_lat..=max_lat).contains(&rlat) && (min_lon..=max_lon).contains(&rlon)
        });
        assert!(hit, "no decoded ring bbox contains the Rome probe point");
    }
}
