//! datagen — regenerate `src/coast_data.rs::LAND_DATA` from a Natural Earth GeoJSON
//! file (public domain, `ne_110m_land.geojson` from nvkelso/natural-earth-vector).
//!
//! Usage (from the crate root):
//!
//! ```text
//! cargo run --example datagen -- data/ne_110m_land.geojson
//! ```
//!
//! The tool extracts **every** polygon ring from **every** feature's geometry (both
//! `Polygon` — exterior ring first, then holes — and `MultiPolygon`), encodes them, and
//! rewrites the region of `src/coast_data.rs` between the `// @generated BEGIN` /
//! `// @generated END` markers in place. The rewrite is idempotent.
//!
//! # Encoding (exact inverse of the decoder in src/coast.rs — see its module docs)
//!
//! 1. Vertices are `(lat_deg, lon_deg)` quantized to centidegrees:
//!    `c = round(deg * 100)` as `i64`.
//! 2. Stream layout (before base85):
//!    `varint ring_count`, then per ring `varint vertex_count`, then per vertex two
//!    zigzag varint deltas (`d_lat` first, then `d_lon`) relative to the previous
//!    vertex of the same ring (first vertex relative to 0).
//! 3. Varints: 7 payload bits per byte, least-significant group first, high bit =
//!    continuation.
//! 4. Zigzag for i64 deltas: `(n << 1) ^ (n >> 63)` (decoder: `(v >> 1) ^ -(v & 1)`).
//! 5. Base85 (Z85 alphabet `0-9a-zA-Z.-:+=^!/*?&<>()[]{}@%$#` — no quote, backslash,
//!    or newline, so the text embeds verbatim in a Rust string literal): the byte
//!    stream is encoded in 4-byte groups read as big-endian `u32`, each emitted as 5
//!    digits most-significant-first. A final partial group of `n` bytes (1..=3) is
//!    zero-padded to 4 bytes and only its first `n + 1` digits are emitted, so the
//!    encoding is always `5*k` or `5*k + (2..=4)` characters long and round-trips
//!    exactly (the decoder completes a short final group with the maximum digit, 84,
//!    which provably recovers the original bytes; see src/coast.rs).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const ALPHABET: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";

const BEGIN_MARKER: &str = "// @generated BEGIN";
const END_MARKER: &str = "// @generated END";

const SIZE_BUDGET: usize = 40 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("datagen: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arg = env::args()
        .nth(1)
        .unwrap_or_else(|| "data/ne_110m_land.geojson".to_string());
    let geojson_path = PathBuf::from(&arg);
    let input = fs::read_to_string(&geojson_path)
        .map_err(|e| format!("cannot read {}: {e}", geojson_path.display()))?;
    let root: serde_json::Value =
        serde_json::from_str(&input).map_err(|e| format!("invalid GeoJSON: {e}"))?;

    let rings = extract_rings(&root)?;
    if rings.is_empty() {
        return Err("no polygon rings found in input".to_string());
    }
    let vertices: usize = rings.iter().map(|r| r.len()).sum();

    let stream = encode_rings(&rings);
    let encoded = base85_encode(&stream);

    println!(
        "datagen: {} features -> {} rings, {} vertices, {} stream bytes, LAND_DATA = {} chars",
        count_features(&root),
        rings.len(),
        vertices,
        stream.len(),
        encoded.len()
    );
    if encoded.len() > SIZE_BUDGET {
        return Err(format!(
            "encoded LAND_DATA is {} bytes, over the {SIZE_BUDGET} byte budget (revisit quantization, PLAN.md §7)",
            encoded.len()
        ));
    }

    let target = find_coast_data(&geojson_path)?;
    rewrite_coast_data(&target, &encoded)?;
    println!(
        "datagen: rewrote {} between markers ({} bytes, budget {} bytes)",
        target.display(),
        encoded.len(),
        SIZE_BUDGET
    );
    Ok(())
}

/// Count top-level features (for the summary line only).
fn count_features(root: &serde_json::Value) -> usize {
    root.get("features")
        .and_then(|f| f.as_array())
        .map_or(0, |a| a.len())
}

/// Collect (lat, lon) rings from every feature, handling Polygon and
/// MultiPolygon. Every ring is validated against the wire format's and the
/// rasterizer's data contracts (see below) — a wrong source file must fail
/// loudly here, not corrupt the committed map.
fn extract_rings(root: &serde_json::Value) -> Result<Vec<Vec<(f64, f64)>>, String> {
    let mut rings = Vec::new();
    let features = match root.get("features").and_then(|f| f.as_array()) {
        Some(f) => f,
        None => return Ok(rings),
    };
    for (fi, feature) in features.iter().enumerate() {
        let Some(geom) = feature.get("geometry") else {
            continue;
        };
        match geom.get("type").and_then(|t| t.as_str()) {
            Some("Polygon") => push_rings(geom.get("coordinates"), &mut rings)
                .map_err(|e| format!("feature {fi}: {e}"))?,
            Some("MultiPolygon") => {
                if let Some(polys) = geom.get("coordinates").and_then(|c| c.as_array()) {
                    for poly in polys {
                        push_rings(Some(poly), &mut rings)
                            .map_err(|e| format!("feature {fi}: {e}"))?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(rings)
}

/// Append every ring of one Polygon's `coordinates` (array of rings of
/// [lon, lat] points), validating:
/// - every point is a numeric [lon, lat] pair within [-180..180] x [-90..90]
///   (malformed points abort instead of being dropped silently);
/// - rings carry at least 3 vertices;
/// - the rasterizer's data contract: non-horizontal edges span at most 180°
///   of longitude — the source must already be split at the antimeridian
///   (Natural Earth is). Horizontal wide edges are exempt (Antarctica's
///   pole closure at lat -90 can never cross a scanline row).
fn push_rings(
    coords: Option<&serde_json::Value>,
    out: &mut Vec<Vec<(f64, f64)>>,
) -> Result<(), String> {
    let Some(rings) = coords.and_then(|c| c.as_array()) else {
        return Ok(());
    };
    for (ri, ring) in rings.iter().enumerate() {
        let points = ring
            .as_array()
            .ok_or_else(|| format!("ring {ri}: coordinates must be an array"))?;
        let mut parsed: Vec<(f64, f64)> = Vec::with_capacity(points.len());
        for (pi, p) in points.iter().enumerate() {
            let arr = p
                .as_array()
                .ok_or_else(|| format!("ring {ri} point {pi}: expected a [lon, lat] array"))?;
            let lon = arr
                .first()
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("ring {ri} point {pi}: longitude must be a number"))?;
            let lat = arr
                .get(1)
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("ring {ri} point {pi}: latitude must be a number"))?;
            if !(-90.0..=90.0).contains(&lat) {
                return Err(format!(
                    "ring {ri} point {pi}: latitude {lat} outside [-90, 90]"
                ));
            }
            if !(-180.0..=180.0).contains(&lon) {
                return Err(format!(
                    "ring {ri} point {pi}: longitude {lon} outside [-180, 180] \
                     (is the source split/normalized at the antimeridian?)"
                ));
            }
            parsed.push((lat, lon));
        }
        if parsed.len() < 3 {
            return Err(format!(
                "ring {ri}: only {} vertices (< 3) — degenerate ring",
                parsed.len()
            ));
        }
        let n = parsed.len();
        for i in 0..n {
            let (a, b) = (parsed[i], parsed[(i + 1) % n]);
            let horizontal = (a.0 - b.0).abs() < 1e-9;
            let dlon = (a.1 - b.1).abs();
            if !horizontal && dlon > 180.0 {
                return Err(format!(
                    "ring {ri}: edge spans {dlon:.2} deg of longitude (lat {} -> {}) \
                     — source is not split at the antimeridian; the rasterizer would \
                     silently mis-render it. Split the ring at ±180 first.",
                    a.0, b.0
                ));
            }
        }
        out.push(parsed);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Encoder (inverse of the decoder in src/coast.rs — see module docs).
// ---------------------------------------------------------------------

fn encode_rings(rings: &[Vec<(f64, f64)>]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, rings.len() as u64);
    for ring in rings {
        put_varint(&mut out, ring.len() as u64);
        let (mut lat_c, mut lon_c): (i64, i64) = (0, 0);
        for &(lat, lon) in ring {
            let l = (lat * 100.0).round() as i64;
            let o = (lon * 100.0).round() as i64;
            put_zigzag(&mut out, l - lat_c);
            put_zigzag(&mut out, o - lon_c);
            lat_c = l;
            lon_c = o;
        }
    }
    out
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
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

fn put_zigzag(out: &mut Vec<u8>, d: i64) {
    put_varint(out, ((d << 1) ^ (d >> 63)) as u64);
}

fn base85_encode(bytes: &[u8]) -> String {
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
            *d = ALPHABET[(v % 85) as usize];
            v /= 85;
        }
        let n = chunk.len();
        out.push_str(std::str::from_utf8(&digits[..n + 1]).unwrap());
    }
    out
}

// ---------------------------------------------------------------------
// Splicing into src/coast_data.rs.
// ---------------------------------------------------------------------

/// Locate src/coast_data.rs: from the cwd, else by walking up from the input file.
fn find_coast_data(geojson_path: &Path) -> Result<PathBuf, String> {
    if Path::new("src/coast_data.rs").is_file() {
        return Ok(PathBuf::from("src/coast_data.rs"));
    }
    for dir in geojson_path.ancestors().skip(1) {
        let candidate = dir.join("src/coast_data.rs");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("cannot locate src/coast_data.rs (run from the crate root)".to_string())
}

/// Replace the marker-delimited region of `target` with the new LAND_DATA static.
fn rewrite_coast_data(target: &Path, encoded: &str) -> Result<(), String> {
    let content =
        fs::read_to_string(target).map_err(|e| format!("cannot read {}: {e}", target.display()))?;
    let start = content.find(BEGIN_MARKER).ok_or("BEGIN marker not found")?;
    let end = content.find(END_MARKER).ok_or("END marker not found")?;
    if end < start {
        return Err("END marker precedes BEGIN marker".to_string());
    }
    let updated = format!(
        "{}\n#[rustfmt::skip]\npub static LAND_DATA: &str = \"{}\";\n{}",
        &content[..start + BEGIN_MARKER.len()],
        encoded,
        &content[end..]
    );
    fs::write(target, updated).map_err(|e| format!("cannot write {}: {e}", target.display()))
}
