//! Dot canvas, braille packing, and geographic scanline land fill. See INTERFACES.md.
//!
//! The canvas is a bit-per-dot grid (`dy = 0` at the top = north, `dx` growing east).
//! Each 2x4 block of dots packs into one Unicode braille character (U+2800..U+28FF).
//!
//! Land fill is a geographic scanline (PLAN §8): in the Kavrayskiy VII projection every
//! parallel is a straight horizontal line, so each dot *row* lies on exactly one latitude.
//! For each row we find the longitudes where the (rotated, dateline-split) polygon edges
//! cross that latitude, sort them, and fill between alternate pairs — projecting the span
//! longitudes to dot columns with the forward x formula (x is linear in longitude at a
//! fixed latitude).

use crate::coast;
use crate::geo;
use std::f64::consts::PI;

/// Braille dot bits per cell position: `CELL_BITS[col][row]`, rows top-to-bottom.
/// dot1..dot8 = 0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80: dots 1-3 are the left column
/// (top-to-bottom), dot 7 the left bottom; dots 4-6 the right column, dot 8 right bottom.
const CELL_BITS: [[u8; 4]; 2] = [
    [0x01, 0x02, 0x04, 0x40], // left column: dots 1, 2, 3, 7
    [0x08, 0x10, 0x20, 0x80], // right column: dots 4, 5, 6, 8
];

/// The braille pattern char for an 8-dot mask: `'\u{2800}' | (mask & 0xFF)`.
pub fn braille(mask: u8) -> char {
    char::from_u32(0x2800 + (mask as u32 & 0xFF)).unwrap()
}

/// Bit-per-dot canvas, row-major, `dots_w * dots_h` dots packed 8 per byte.
pub struct Canvas {
    pub dots_w: usize,
    pub dots_h: usize,
    masks: Vec<u8>,
}

impl Canvas {
    pub fn new(dots_w: usize, dots_h: usize) -> Self {
        let bits = dots_w.saturating_mul(dots_h);
        Canvas {
            dots_w,
            dots_h,
            masks: vec![0u8; bits.div_ceil(8)],
        }
    }

    #[allow(dead_code)] // public canvas API; exercised by tests
    pub fn clear(&mut self) {
        self.masks.fill(0);
    }

    /// Set the dot at (dx, dy). Out-of-range coordinates are ignored.
    pub fn set_dot(&mut self, dx: usize, dy: usize) {
        if let Some(bit) = self.bit_index(dx, dy) {
            self.masks[bit / 8] |= 1 << (bit % 8);
        }
    }

    /// Whether the dot at (dx, dy) is set. Out-of-range coordinates read as false.
    pub fn dot_set(&self, dx: usize, dy: usize) -> bool {
        match self.bit_index(dx, dy) {
            Some(bit) => self.masks[bit / 8] & (1 << (bit % 8)) != 0,
            None => false,
        }
    }

    /// Pack the 2x4 dots of character cell (cx, cy) into a braille mask.
    /// Out-of-range dots contribute nothing.
    pub fn cell_mask(&self, cx: usize, cy: usize) -> u8 {
        let mut mask = 0u8;
        for (col, bits) in CELL_BITS.iter().enumerate() {
            for (row, bit) in bits.iter().enumerate() {
                if self.dot_set(2 * cx + col, 4 * cy + row) {
                    mask |= bit;
                }
            }
        }
        mask
    }

    /// The braille character for cell (cx, cy).
    #[allow(dead_code)] // public canvas API; exercised by tests
    pub fn cell_char(&self, cx: usize, cy: usize) -> char {
        braille(self.cell_mask(cx, cy))
    }

    fn bit_index(&self, dx: usize, dy: usize) -> Option<usize> {
        if dx >= self.dots_w || dy >= self.dots_h {
            None
        } else {
            Some(dy * self.dots_w + dx)
        }
    }
}

/// A polygon edge in rotated frame space: longitudes normalized to (-PI, PI],
/// latitudes in radians.
#[derive(Clone, Copy)]
struct Edge {
    lat0: f64,
    lon0: f64,
    lat1: f64,
    lon1: f64,
}

/// Any angle in radians to (-PI, PI].
fn normalize_lon_r(lon: f64) -> f64 {
    let mut l = lon.rem_euclid(2.0 * PI);
    if l > PI {
        l -= 2.0 * PI;
    }
    l
}

/// Kavrayskiy VII forward x (PLAN §6.1) for a longitude already rotated by the central
/// meridian and normalized to (-PI, PI]. Mirrors `geo::forward`'s x; kept local so the
/// raster math is self-contained: x = 3/2 · lon · sqrt(1/3 − (lat/π)²), and x is linear
/// in longitude at a fixed latitude.
fn kav_x(lon_rel: f64, lat: f64) -> f64 {
    1.5 * lon_rel * (1.0 / 3.0 - (lat / PI).powi(2)).sqrt()
}

/// Build the rings' edges in **raw source coordinates** (latitudes and longitudes
/// in radians, no rotation, no normalization).
///
/// Data contract: rings are pre-split at the antimeridian, as Natural Earth
/// provides them — every edge spans at most 180° of longitude, and rings that
/// touch the dateline carry sliver edges along ±180° that close them properly
/// in raw space. (The committed dataset has exactly one wider edge: Antarctica's
/// pole closure at latitude −90, horizontal, which can never cross a scanline
/// row and is therefore inert.) Working purely in raw space makes the fill's
/// semantics identical to a plain ray-cast point-in-polygon test on the source
/// rings — verified against exactly such an oracle in the tests.
fn build_edges(rings: &[coast::Ring], edges: &mut Vec<Edge>) {
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            edges.push(Edge {
                lat0: a.0.to_radians(),
                lon0: a.1.to_radians(),
                lat1: b.0.to_radians(),
                lon1: b.1.to_radians(),
            });
        }
    }
}

/// Fill one even-odd span of raw longitudes `[a, b]` (a < b, both in radians) at
/// latitude `lat`, under central meridian `lambda0`: rotate the span into
/// central-meridian-relative space as one continuous interval, split it at every
/// 2*PI window boundary so each piece lies within (-PI, PI], project the piece
/// ends to fractional dot columns, and set every dot whose center falls inside.
/// A full-window piece (e.g. a polar-cap span of ±180°) fills the whole row.
#[allow(clippy::too_many_arguments)] // span + frame geometry; a context struct would be ceremony
fn fill_span(
    canvas: &mut Canvas,
    a: f64,
    b: f64,
    lat: f64,
    lambda0: f64,
    s: f64,
    half_w: f64,
    dw: usize,
    dy: usize,
) {
    let two_pi = 2.0 * PI;
    let eps = 1e-9;
    // Rotate into the frame's space as one continuous interval (width <= 2*PI).
    let mut start = normalize_lon_r(a - lambda0);
    let end_rel = start + (b - a);
    while start < end_rel - eps {
        // Window containing `start`: [w*2PI - PI, w*2PI + PI).
        let w = ((start + PI) / two_pi).floor();
        let win_hi = (w + 1.0) * two_pi - PI;
        let e = end_rel.min(win_hi);
        let l0 = start - w * two_pi;
        let l1 = e - w * two_pi;
        if l1 - l0 >= two_pi - 1e-6 {
            // Whole world at this latitude: fill the row.
            for dx in 0..dw {
                canvas.set_dot(dx, dy);
            }
            break;
        }
        let f0 = kav_x(l0, lat) * s + half_w - 0.5;
        let f1 = kav_x(l1, lat) * s + half_w - 0.5;
        let (f0, f1) = if f0 <= f1 { (f0, f1) } else { (f1, f0) };
        let d0 = ((f0 - eps).ceil() as i64).max(0);
        let d1 = ((f1 + eps).floor() as i64).min(dw as i64 - 1);
        for dx in d0..=d1 {
            canvas.set_dot(dx as usize, dy);
        }
        start = e;
    }
}

/// Geographic scanline fill (PLAN §8): for each dot row (parallels are straight
/// lines in this projection, so one row = one latitude) collect the longitudes
/// where the rings' edges cross the row latitude, sort them, and fill between
/// alternate pairs — the classic even-odd rule, in raw source coordinates.
/// Because Natural Earth rings are closed in raw space (see [`build_edges`]),
/// this is exactly ray-cast point-in-polygon parity, with −180° sorting west of
/// everything and +180° east of everything, matching the dateline slivers.
/// Spans are rotated to the central meridian only when projected onto the dot
/// grid; a span wider than the map window wraps via [`fill_span`].
pub fn draw_land(canvas: &mut Canvas, frame: &geo::MapFrame, rings: &[coast::Ring]) {
    let (dw, dh) = (frame.dots_w, frame.dots_h);
    if dw == 0 || dh == 0 || frame.x_max <= frame.x_min || frame.y_max <= frame.y_min {
        return;
    }

    let mut edges: Vec<Edge> = Vec::new();
    build_edges(rings, &mut edges);
    if edges.is_empty() {
        return;
    }

    let lat_eps = 1e-9; // nudge rows that exactly hit a vertex latitude
    let s = frame.scale();
    let half_w = dw as f64 / 2.0;
    let mut crossings: Vec<f64> = Vec::new();

    for dy in 0..dh {
        // Row latitude from the frame's dot-center y (dy = 0 is the top/north row).
        let lat = frame.dot_center(0, dy).1;
        let lat_t = lat + lat_eps;

        crossings.clear();
        for e in &edges {
            // Standard half-open ray-cast rule: count each edge once.
            if (e.lat0 <= lat_t && lat_t < e.lat1) || (e.lat1 <= lat_t && lat_t < e.lat0) {
                let t = (lat_t - e.lat0) / (e.lat1 - e.lat0);
                crossings.push(e.lon0 + t * (e.lon1 - e.lon0));
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

        for pair in crossings.chunks(2) {
            if let [c0, c1] = pair {
                fill_span(canvas, *c0, *c1, lat, frame.lambda0, s, half_w, dw, dy);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coast;
    use std::f64::consts::PI;

    /// Exact map-oval bbox (INTERFACES.md geo section): x in ±sqrt(3)·PI/2, y in ±PI/2.
    pub(crate) fn test_frame(dw: usize, dh: usize, lambda0_deg: f64) -> geo::MapFrame {
        let x_half = PI * 3.0f64.sqrt() / 2.0;
        geo::MapFrame {
            dots_w: dw,
            dots_h: dh,
            lambda0: lambda0_deg.to_radians(),
            x_min: -x_half,
            x_max: x_half,
            y_min: -PI / 2.0,
            y_max: PI / 2.0,
        }
    }

    /// Row latitude for a dot row — the frame's own dot-center mapping (dy = 0 = north).
    pub(crate) fn row_lat(frame: &geo::MapFrame, dy: usize) -> f64 {
        frame.dot_center(0, dy).1
    }

    /// Nearest dot for a geographic probe point (degrees), using the frame's mapping.
    pub(crate) fn dot_for(frame: &geo::MapFrame, lon_deg: f64, lat_deg: f64) -> (usize, usize) {
        let (x, y) = geo::forward(lon_deg.to_radians(), lat_deg.to_radians(), frame.lambda0);
        let s = frame.scale();
        let fx = x * s + frame.dots_w as f64 / 2.0 - 0.5;
        let fy = frame.dots_h as f64 / 2.0 - y * s - 0.5;
        let dx = fx.round().clamp(0.0, frame.dots_w as f64 - 1.0) as usize;
        let dy = fy.round().clamp(0.0, frame.dots_h as f64 - 1.0) as usize;
        (dx, dy)
    }

    /// Closed ring from (lat_deg, lon_deg) vertices.
    fn ring(pts: &[(f64, f64)]) -> coast::Ring {
        pts.to_vec()
    }

    fn assert_dot(c: &Canvas, f: &geo::MapFrame, lon: f64, lat: f64, want: bool, what: &str) {
        let (dx, dy) = dot_for(f, lon, lat);
        assert_eq!(
            c.dot_set(dx, dy),
            want,
            "{what}: dot ({dx},{dy}) for probe (lon {lon}°, lat {lat}°)"
        );
    }

    #[test]
    fn braille_bit_layout() {
        assert_eq!(braille(0x00), '\u{2800}');
        assert_eq!(braille(0x01), '\u{2801}'); // dot 1
        assert_eq!(braille(0x02), '\u{2802}'); // dot 2
        assert_eq!(braille(0x04), '\u{2804}'); // dot 3
        assert_eq!(braille(0x08), '\u{2808}'); // dot 4
        assert_eq!(braille(0x10), '\u{2810}'); // dot 5
        assert_eq!(braille(0x20), '\u{2820}'); // dot 6
        assert_eq!(braille(0x40), '\u{2840}'); // dot 7
        assert_eq!(braille(0x80), '\u{2880}'); // dot 8
        assert_eq!(braille(0xFF), '\u{28FF}'); // all dots
    }

    #[test]
    fn canvas_dot_and_cell_packing() {
        let mut c = Canvas::new(4, 8); // 2x2 character cells
        assert_eq!(c.cell_mask(0, 0), 0);
        assert_eq!(c.cell_char(0, 0), '\u{2800}');
        assert!(!c.dot_set(0, 0));

        // Out-of-range reads are false and out-of-range writes are ignored (no panic).
        assert!(!c.dot_set(4, 0));
        assert!(!c.dot_set(0, 8));
        c.set_dot(100, 100);
        assert_eq!(c.cell_mask(0, 0), 0);

        // Each dot of a 2x4 cell maps to the documented bit.
        for (col, bits_col) in CELL_BITS.iter().enumerate() {
            for (row, bit) in bits_col.iter().enumerate() {
                let mut c = Canvas::new(2, 4);
                c.set_dot(col, row);
                assert_eq!(c.cell_mask(0, 0), *bit, "dot (col {col}, row {row})");
            }
        }

        // A full 2x4 cell packs to 0xFF; the neighbouring cell stays empty.
        for dx in 0..2 {
            for dy in 0..4 {
                c.set_dot(dx, dy);
            }
        }
        assert_eq!(c.cell_mask(0, 0), 0xFF);
        assert_eq!(c.cell_char(0, 0), '\u{28FF}');
        assert_eq!(c.cell_mask(1, 1), 0);

        // Bottom-right dot of the bottom-right cell is dot 8 (0x80).
        c.set_dot(3, 7);
        assert_eq!(c.cell_mask(1, 1), 0x80);
        assert!(c.dot_set(3, 7));

        // clear() resets every dot.
        c.clear();
        assert_eq!(c.cell_mask(0, 0), 0);
        assert_eq!(c.cell_mask(1, 1), 0);
        assert!(!c.dot_set(3, 7));
        assert!(!c.dot_set(0, 0));
    }

    #[test]
    fn scanline_fills_axis_aligned_rectangle() {
        let f = test_frame(240, 120, 0.0);
        let mut c = Canvas::new(f.dots_w, f.dots_h);
        let rect = ring(&[(-10.0, -20.0), (-10.0, 20.0), (10.0, 20.0), (10.0, -20.0)]);
        draw_land(&mut c, &f, &[rect]);

        // Point probes comfortably inside / outside the box.
        assert_dot(&c, &f, 0.0, 0.0, true, "box center");
        assert_dot(&c, &f, 15.0, 5.0, true, "inside east");
        assert_dot(&c, &f, -15.0, -5.0, true, "inside west");
        assert_dot(&c, &f, 30.0, 0.0, false, "east of the box");
        assert_dot(&c, &f, -30.0, 0.0, false, "west of the box");
        assert_dot(&c, &f, 0.0, 30.0, false, "north of the box");
        assert_dot(&c, &f, 0.0, -30.0, false, "south of the box");
        assert_dot(&c, &f, 170.0, 0.0, false, "far east sea");
        assert_dot(&c, &f, -170.0, 0.0, false, "far west sea");

        // Exhaustive per-dot check with a margin of one dot at every span/band edge
        // (tolerant of inclusive-vs-exclusive rounding at exact boundaries).
        const LAT_M: f64 = 0.75; // half a row pitch (120 rows over 180°)
        const DX_M: f64 = 1.5; // ~one dot column at the equator
        let mut set_count = 0usize;
        for dy in 0..f.dots_h {
            let lat = row_lat(&f, dy);
            let latd = lat.to_degrees();
            let row_in = latd > -10.0 + LAT_M && latd < 10.0 - LAT_M;
            let row_out = latd > 10.0 + LAT_M || latd < -10.0 - LAT_M;
            let k = (1.0 / 3.0 - (lat / PI).powi(2)).sqrt();
            let s = f.scale();
            let half_w = f.dots_w as f64 / 2.0;
            let fx = |lon_deg: f64| 1.5 * lon_deg.to_radians() * k * s + half_w - 0.5;
            let (s0, s1) = (fx(-20.0), fx(20.0));
            for dx in 0..f.dots_w {
                let p = dx as f64;
                let is_set = c.dot_set(dx, dy);
                if is_set {
                    set_count += 1;
                }
                if row_out {
                    assert!(!is_set, "dot ({dx},{dy}) at lat {latd:.2}° must be clear");
                } else if row_in {
                    if p > s0 + DX_M && p < s1 - DX_M {
                        assert!(is_set, "inside dot ({dx},{dy}) at lat {latd:.2}°");
                    }
                    if p < s0 - DX_M || p > s1 + DX_M {
                        assert!(!is_set, "outside dot ({dx},{dy}) at lat {latd:.2}°");
                    }
                }
            }
        }
        // Something was filled, but only a small fraction of the map.
        assert!(set_count > 0);
        assert!(set_count < f.dots_w * f.dots_h / 4);
    }

    #[test]
    fn scanline_fills_concave_u_shape() {
        let f = test_frame(240, 120, 0.0);
        let mut c = Canvas::new(f.dots_w, f.dots_h);
        // U shape: two arms (lat -20..20 at lon -30..-10 and 10..30) joined by a base
        // (lat -20..-10 across the whole width); the notch between the arms is sea.
        let u = ring(&[
            (-20.0, -30.0),
            (-20.0, 30.0),
            (20.0, 30.0),
            (20.0, 10.0),
            (-10.0, 10.0),
            (-10.0, -10.0),
            (20.0, -10.0),
            (20.0, -30.0),
        ]);
        draw_land(&mut c, &f, &[u]);

        // A row through both arms and the notch: arms set, gap clear.
        for lon in [-28.0, -20.0, -12.0, 12.0, 20.0, 28.0] {
            assert_dot(&c, &f, lon, 0.0, true, "arm at lat 0");
        }
        for lon in [-8.0, 0.0, 8.0] {
            assert_dot(&c, &f, lon, 0.0, false, "notch gap at lat 0");
        }

        // The base below the notch is solid across.
        assert_dot(&c, &f, 0.0, -15.0, true, "base center");
        assert_dot(&c, &f, 25.0, -15.0, true, "base east");
        assert_dot(&c, &f, -25.0, -15.0, true, "base west");

        // Higher up, the notch is still sea while the arms are land.
        assert_dot(&c, &f, 0.0, 15.0, false, "notch interior high");
        assert_dot(&c, &f, -5.0, 10.0, false, "notch interior");
        assert_dot(&c, &f, -20.0, 15.0, true, "left arm high");
        assert_dot(&c, &f, 20.0, 15.0, true, "right arm high");

        // Above and around the whole shape: sea.
        assert_dot(&c, &f, 0.0, 25.0, false, "above the notch");
        assert_dot(&c, &f, 40.0, 0.0, false, "east of the right arm");
        assert_dot(&c, &f, -40.0, 0.0, false, "west of the left arm");
        assert_dot(&c, &f, 0.0, -40.0, false, "south of the base");
    }

    #[test]
    fn scanline_dateline_ring_fills_near_dateline_only() {
        let f = test_frame(240, 120, 0.0);
        let mut c = Canvas::new(f.dots_w, f.dots_h);
        // Land at lon 170..190 (i.e. 170..180 plus -180..-170) and lat -10..10,
        // expressed in the data contract's pre-split form: one polygon per side
        // of the antimeridian (Natural Earth splits polygons this way).
        let east = ring(&[(-10.0, 170.0), (-10.0, 180.0), (10.0, 180.0), (10.0, 170.0)]);
        let west = ring(&[
            (-10.0, -180.0),
            (-10.0, -170.0),
            (10.0, -170.0),
            (10.0, -180.0),
        ]);
        draw_land(&mut c, &f, &[east, west]);

        // Filled right up to the dateline on both sides…
        assert_dot(&c, &f, 175.0, 0.0, true, "east of the dateline");
        assert_dot(&c, &f, 179.0, 0.0, true, "dateline east edge");
        assert_dot(&c, &f, -175.0, 0.0, true, "west of the dateline");
        assert_dot(&c, &f, -179.0, 0.0, true, "dateline west edge");
        assert_dot(&c, &f, 175.0, 5.0, true, "north-east lobe");
        assert_dot(&c, &f, -175.0, -5.0, true, "south-west lobe");

        // …and nowhere else: the middle of the map must stay sea.
        assert_dot(&c, &f, 0.0, 0.0, false, "map middle");
        assert_dot(&c, &f, 90.0, 0.0, false, "Indian Ocean");
        assert_dot(&c, &f, -90.0, 0.0, false, "Americas");
        assert_dot(&c, &f, 160.0, 0.0, false, "just west of the ring");
        assert_dot(&c, &f, -160.0, 0.0, false, "just east of the ring");
        assert_dot(&c, &f, 175.0, 20.0, false, "north of the ring");
        assert_dot(&c, &f, 175.0, -20.0, false, "south of the ring");
    }

    #[test]
    fn rotated_center_moves_the_dateline_split() {
        // A small ring near Greenwich, rendered with a central meridian of 180°:
        // the ring must appear at the far edges of the map (the rotated seam).
        let f = test_frame(240, 120, 180.0);
        let mut c = Canvas::new(f.dots_w, f.dots_h);
        let r = ring(&[(-10.0, -10.0), (-10.0, 10.0), (10.0, 10.0), (10.0, -10.0)]);
        draw_land(&mut c, &f, &[r]);

        assert_dot(&c, &f, 0.0, 0.0, true, "ring center at lon 0");
        assert_dot(&c, &f, 5.0, 5.0, true, "inside NE corner");
        assert_dot(&c, &f, -5.0, -5.0, true, "inside SW corner");
        assert_dot(&c, &f, 90.0, 0.0, false, "map middle (lon 90)");
        assert_dot(&c, &f, -90.0, 0.0, false, "map middle (lon -90)");
        assert_dot(&c, &f, 0.0, 30.0, false, "north of the ring");
    }

    #[test]
    fn degenerate_canvases_do_not_panic() {
        // 0x0 grid.
        let f0 = test_frame(0, 0, 0.0);
        let mut c0 = Canvas::new(0, 0);
        draw_land(
            &mut c0,
            &f0,
            &[ring(&[(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)])],
        );

        // 1x1 dot canvas: one row at the north pole, one column.
        let f1 = test_frame(1, 1, 0.0);
        let mut c1 = Canvas::new(1, 1);
        draw_land(
            &mut c1,
            &f1,
            &[ring(&[
                (-10.0, -20.0),
                (-10.0, 20.0),
                (10.0, 20.0),
                (10.0, -20.0),
            ])],
        );
        c1.set_dot(0, 0);
        assert!(c1.dot_set(0, 0));
        assert!(!c1.dot_set(1, 1));

        // 200x60 dots (PLAN §14.1 size) with a couple of rings — must not panic
        // and must fill something.
        let f2 = test_frame(200, 60, 30.0);
        let mut c2 = Canvas::new(200, 60);
        draw_land(
            &mut c2,
            &f2,
            &[
                ring(&[(-10.0, -20.0), (-10.0, 20.0), (10.0, 20.0), (10.0, -20.0)]),
                ring(&[
                    (-80.0, -179.0),
                    (-80.0, 179.0),
                    (-70.0, 179.0),
                    (-70.0, -179.0),
                ]),
                ring(&[(60.0, 150.0), (70.0, 170.0), (65.0, -170.0)]),
            ],
        );
        let any = (0..c2.dots_h).any(|dy| (0..c2.dots_w).any(|dx| c2.dot_set(dx, dy)));
        assert!(any, "some dots must be set on the 200x60 canvas");

        // Cell reads on a partially-filled canvas stay in range and well-formed.
        for cy in 0..c2.dots_h.div_ceil(4) {
            for cx in 0..c2.dots_w.div_ceil(2) {
                let ch = c2.cell_char(cx, cy);
                assert!(('\u{2800}'..='\u{28FF}').contains(&ch));
            }
        }
    }

    /// Real-data integration test: decode the embedded coastlines and fill a
    /// full-size canvas.
    #[test]
    fn real_data_integration() {
        let rings = coast::decode(crate::coast_data::LAND_DATA);
        assert!(!rings.is_empty(), "embedded data must decode");

        let f = test_frame(200, 60, 0.0);
        let mut c = Canvas::new(200, 60);
        draw_land(&mut c, &f, &rings); // must not panic on real polygons
        let n = (0..c.dots_h)
            .flat_map(|dy| (0..c.dots_w).map(move |dx| (dx, dy)))
            .filter(|&(dx, dy)| c.dot_set(dx, dy))
            .count();
        assert!(n > 0, "real data must fill some dots");
        // Land covers ~29% of the globe; allow a generous band for the projection's
        // area distortion and this coarse grid.
        let frac = n as f64 / (200.0 * 60.0);
        assert!(
            frac > 0.05 && frac < 0.55,
            "implausible land fraction {frac:.3}"
        );
    }
}

/// Cross-validation of the scanline fill against an independent oracle:
/// even-odd point-in-polygon computed on the *raw* source rings (no rotation,
/// no normalization — pre-split rings are simple polygons in source space, so
/// the dateline needs no special handling there). Any dot whose center is not
/// close to a coastline must agree with the oracle, at several central
/// meridians. This is the regression net that caught the Arctic inversion and
/// the Antarctic/Pacific fill corruption.
#[cfg(test)]
mod pip_oracle {
    use super::tests::{dot_for, test_frame};
    use crate::coast;
    use crate::coast_data;
    use crate::raster::{draw_land, Canvas};

    /// Even-odd PIP on raw rings: ray cast eastward from (lat, lon).
    fn is_land_raw(rings: &[coast::Ring], lat: f64, lon: f64) -> bool {
        let mut inside = false;
        for ring in rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if (a.0 <= lat && lat < b.0) || (b.0 <= lat && lat < a.0) {
                    let t = (lat - a.0) / (b.0 - a.0);
                    let x = a.1 + t * (b.1 - a.1);
                    if x > lon {
                        inside = !inside;
                    }
                }
            }
        }
        inside
    }

    /// Rough distance (degrees) from a point to the nearest ring vertex —
    /// used to exempt coastline-adjacent dots from exact agreement.
    fn nearest_vertex_deg(rings: &[coast::Ring], lat: f64, lon: f64) -> f64 {
        let mut best = f64::MAX;
        for ring in rings {
            for &(vlat, vlon) in ring {
                let d = ((vlat - lat).powi(2) + ((vlon - lon) * 0.55).powi(2)).sqrt();
                if d < best {
                    best = d;
                }
            }
        }
        best
    }

    #[test]
    fn scanline_agrees_with_raw_pip() {
        let rings = coast::decode(coast_data::LAND_DATA);
        for &lambda0 in &[0.0, 90.0, 180.0] {
            let f = test_frame(200, 100, lambda0);
            let mut c = Canvas::new(200, 100);
            draw_land(&mut c, &f, &rings);

            let mut checked = 0usize;
            let mut mismatches = 0usize;
            for dy in (0..100).step_by(2) {
                for dx in (0..200).step_by(2) {
                    let Some((lon_rad, lat_rad)) = f.dot_to_lonlat(dx, dy) else {
                        continue;
                    };
                    let (lon, lat) = (lon_rad.to_degrees(), lat_rad.to_degrees());
                    let expected = is_land_raw(&rings, lat, lon);
                    let got = c.dot_set(dx, dy);
                    checked += 1;
                    if expected != got {
                        mismatches += 1;
                        let near = nearest_vertex_deg(&rings, lat, lon);
                        assert!(
                            near < 2.0,
                            "dot ({dx},{dy}) = ({lat:.2},{lon:.2}): fill={got} but oracle={expected} \
                             (nearest vertex {near:.2} deg away) at lambda0={lambda0}"
                        );
                    }
                }
            }
            assert!(checked > 2000);
            let frac = mismatches as f64 / checked as f64;
            assert!(
                frac < 0.02,
                "{mismatches}/{checked} mismatches at lambda0={lambda0} (frac {frac:.4})"
            );
        }
    }

    /// High-confidence geography probes, including the user-visible symptoms:
    /// everything north of the Chukotka dateline cut (~66-70N) used to render
    /// inverted, and the far south had corrupted fills. Probes are chosen away
    /// from coasts so row rounding cannot land them on the wrong side.
    #[test]
    fn arctic_and_austral_probes() {
        let rings = coast::decode(coast_data::LAND_DATA);
        let f = test_frame(240, 120, 0.0);
        let mut c = Canvas::new(240, 120);
        draw_land(&mut c, &f, &rings);
        let land = [
            (75.0, -40.0, "Greenland interior"),
            (78.0, -68.0, "NW Greenland"),
            (81.0, -78.0, "Ellesmere interior"),
            (70.5, 100.0, "Siberia north"),
            (62.0, 95.0, "Siberia interior"),
            (40.0, -100.0, "USA interior"),
            (-25.0, 133.0, "Australia interior"),
            (55.0, 10.0, "Denmark"),
            (-80.0, 120.0, "East Antarctic interior"),
        ];
        let sea = [
            (85.0, 0.0, "Arctic ocean"),
            (72.0, 5.0, "Norwegian sea"),
            (70.5, -140.0, "Beaufort sea"),
            (60.0, -52.0, "Labrador sea"),
            (0.0, -140.0, "mid-Pacific"),
            (30.0, -40.0, "mid-Atlantic"),
            (-35.0, 60.0, "Indian ocean"),
            (-58.0, -160.0, "Southern ocean"),
            (-65.0, -175.0, "South Pacific"),
        ];
        for &(lat, lon, what) in &land {
            let (dx, dy) = dot_for(&f, lon, lat);
            assert!(
                c.dot_set(dx, dy),
                "{what} (lat {lat}, lon {lon}) must be land"
            );
        }
        for &(lat, lon, what) in &sea {
            let (dx, dy) = dot_for(&f, lon, lat);
            assert!(
                !c.dot_set(dx, dy),
                "{what} (lat {lat}, lon {lon}) must be sea"
            );
        }
    }
}

/// Trace the rings' edges onto the canvas as a one-dot-wide coastline outline
/// (OR-ed in; callers that want it terrain-colored merge it into the land
/// canvas before styling). Each edge is sampled densely enough (twice per dot
/// pitch along its projected extent) that the outline is visually continuous.
///
/// Edges are straight lines in raw (lat, lon) space; samples are interpolated
/// there and projected individually, which is exact for any central meridian —
/// per-sample rotation is well-defined even when an edge straddles the rotated
/// seam, because each sample lands at its own correct x.
pub fn draw_coastline(canvas: &mut Canvas, frame: &geo::MapFrame, rings: &[coast::Ring]) {
    if frame.dots_w == 0 || frame.dots_h == 0 {
        return;
    }
    let s = frame.scale();
    let half_w = frame.dots_w as f64 / 2.0;
    let half_h = frame.dots_h as f64 / 2.0;
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            // Projected endpoints (dot units) to size the sample count.
            let proj = |lat_deg: f64, lon_deg: f64| {
                let lat = lat_deg.to_radians();
                let x = kav_x(normalize_lon_r(lon_deg.to_radians() - frame.lambda0), lat);
                (x * s + half_w - 0.5, half_h - lat * s - 0.5)
            };
            let (x0, y0) = proj(a.0, a.1);
            let (x1, y1) = proj(b.0, b.1);
            let dist = ((x1 - x0).hypot(y1 - y0)).max(1.0);
            let steps = (dist * 2.0).ceil() as usize;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                let lat_deg = a.0 + t * (b.0 - a.0);
                let lon_deg = a.1 + t * (b.1 - a.1);
                let (fx, fy) = proj(lat_deg, lon_deg);
                let dx = fx.round() as isize;
                let dy = fy.round() as isize;
                if dx >= 0
                    && dy >= 0
                    && (dx as usize) < frame.dots_w
                    && (dy as usize) < frame.dots_h
                {
                    canvas.set_dot(dx as usize, dy as usize);
                }
            }
        }
    }
}

#[cfg(test)]
mod coastline_tests {
    use super::tests::{dot_for, test_frame};
    use super::{draw_coastline, draw_land, Canvas};
    use crate::coast::Ring;

    fn box_ring(lat_min: f64, lon_min: f64, lat_max: f64, lon_max: f64) -> Ring {
        vec![
            (lat_min, lon_min),
            (lat_min, lon_max),
            (lat_max, lon_max),
            (lat_max, lon_min),
            (lat_min, lon_min),
        ]
    }

    #[test]
    fn outline_traces_edges_not_interior() {
        for &lambda0 in &[0.0, 90.0] {
            let f = test_frame(240, 120, lambda0);
            let mut outline = Canvas::new(240, 120);
            draw_coastline(&mut outline, &f, &[box_ring(-10.0, -20.0, 10.0, 20.0)]);

            // Edge midpoints are traced (all four sides).
            for &(lat, lon) in &[(0.0, -20.0), (0.0, 20.0), (-10.0, 0.0), (10.0, 0.0)] {
                let (dx, dy) = dot_for(&f, lon, lat);
                // The traced dot sits within one dot of the ideal edge point.
                let mut hit = false;
                for oy in -1i64..=1 {
                    for ox in -1i64..=1 {
                        let (x, y) = (dx as i64 + ox, dy as i64 + oy);
                        if x >= 0 && y >= 0 && (x as usize) < 240 && (y as usize) < 120 {
                            hit |= outline.dot_set(x as usize, y as usize);
                        }
                    }
                }
                assert!(
                    hit,
                    "edge midpoint ({lat},{lon}) traced at lambda0={lambda0}"
                );
            }

            // The interior is NOT filled by the outline alone (it is a line, not an
            // area): the box center and its 1-dot neighborhood stay clear.
            let (cx, cy) = dot_for(&f, 0.0, 0.0);
            assert!(
                !outline.dot_set(cx, cy),
                "outline must not fill the interior"
            );

            // Far outside: nothing.
            let (ox, oy) = dot_for(&f, 60.0, 0.0);
            assert!(!outline.dot_set(ox, oy));
        }
    }

    #[test]
    fn outline_plus_fill_is_superset_of_fill() {
        // The outline layer must only ever ADD dots to the land layer.
        let f = test_frame(200, 100, 30.0);
        let rings = vec![
            box_ring(-10.0, -20.0, 10.0, 20.0),
            box_ring(30.0, 100.0, 50.0, 140.0),
        ];
        let mut fill = Canvas::new(200, 100);
        draw_land(&mut fill, &f, &rings);
        let mut both = Canvas::new(200, 100);
        draw_land(&mut both, &f, &rings);
        draw_coastline(&mut both, &f, &rings);
        for dy in 0..100 {
            for dx in 0..200 {
                if fill.dot_set(dx, dy) {
                    assert!(
                        both.dot_set(dx, dy),
                        "outline lost a fill dot at ({dx},{dy})"
                    );
                }
            }
        }
        // And the union is strictly larger: an outline dot exists beyond the fill.
        let extra = (0..100)
            .flat_map(|dy| (0..200).map(move |dx| (dx, dy)))
            .filter(|&(dx, dy)| both.dot_set(dx, dy) && !fill.dot_set(dx, dy))
            .count();
        assert!(extra > 0, "outline should add dots the fill missed");
    }
}
