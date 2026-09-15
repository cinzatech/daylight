//! Kavrayskiy VII projection and map-frame math. See PLAN.md §6.1.
//!
//! All angles are radians inside this module, except parameters and results
//! named `*_deg` at the `MapFrame` boundary (degrees there).
//! `forward`/`inverse` work relative to a central meridian `lambda0`; `MapFrame`
//! lays the projected oval over a dot grid (dots are ~square: terminal cells are
//! 1:2 and braille packs 2x4 dots per cell), with `dy = 0` at the top (north) row.

use std::f64::consts::PI;

/// Tolerance for on-the-boundary floating-point comparisons (radians / map units).
const EPS: f64 = 1e-9;

/// Half-width of the map oval at latitude `lat` (radians): |x| at lon offset ±π.
/// `oval_half_width(0.0) = sqrt(3)·π/2` is the bbox half-width; the oval is the
/// ellipse x²/(√3π/2)² + (lat/(π/√3))² = 1 truncated at |lat| ≤ π/2.
fn oval_half_width(lat: f64) -> f64 {
    1.5 * PI * (1.0 / 3.0 - (lat / PI) * (lat / PI)).sqrt()
}

/// Forward projection. lon/lat/lambda0 in radians. The formulas are defined
/// for |lat| < pi·sqrt(1/3) (~103.9°); beyond that the sqrt argument goes
/// negative and x is NaN (a debug assert documents the boundary; MapFrame
/// never projects beyond ±pi/2). Returns (x, y): x = 1.5*(lon-lambda0
/// normalized to (-pi,pi]) * sqrt(1/3 - (lat/pi)^2), y = lat.
pub fn forward(lon: f64, lat: f64, lambda0: f64) -> (f64, f64) {
    debug_assert!(
        (lat / PI) * (lat / PI) < 1.0 / 3.0,
        "forward: |lat| beyond ~103.9 deg leaves the projection's sqrt domain"
    );
    let lon_rel = normalize_lon(lon - lambda0);
    (
        1.5 * lon_rel * (1.0 / 3.0 - (lat / PI) * (lat / PI)).sqrt(),
        lat,
    )
}

/// Inverse: returns (lon-lambda0 in (-pi,pi], lat). Total for |y| <= pi/2
/// (sqrt arg never zero: (lat/pi)^2 <= 1/4 < 1/3); |y| beyond pi·sqrt(1/3)
/// (~103.9°) leaves the sqrt domain (a debug assert documents the boundary).
pub fn inverse(x: f64, y: f64) -> (f64, f64) {
    debug_assert!(
        (y / PI) * (y / PI) < 1.0 / 3.0,
        "inverse: |y| beyond ~103.9 deg leaves the projection's sqrt domain"
    );
    let lat = y;
    let lon_rel = 2.0 * x / (3.0 * (1.0 / 3.0 - (y / PI) * (y / PI)).sqrt());
    (normalize_lon(lon_rel), lat)
}

/// Any radians -> (-pi, pi]. The lower boundary -pi maps to +pi. Non-finite
/// input (NaN, ±inf) returns 0.0 so the documented total range always holds.
pub fn normalize_lon(lon: f64) -> f64 {
    if !lon.is_finite() {
        return 0.0;
    }
    let two_pi = 2.0 * PI;
    let mut l = lon % two_pi;
    if l <= -PI {
        l += two_pi;
    } else if l > PI {
        l -= two_pi;
    }
    l
}

/// Layout of the map oval over a dot grid (dots are ~square: cells are 1:2, braille 2x4).
#[derive(Clone, Copy, Debug)]
pub struct MapFrame {
    pub dots_w: usize,
    pub dots_h: usize,
    pub lambda0: f64, // radians
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

impl MapFrame {
    /// Uniform scale; the whole oval bbox x ∈ ±sqrt(3)·π/2, y ∈ ±π/2 fits, centered.
    pub fn new(dots_w: usize, dots_h: usize, lambda0_deg: f64) -> Self {
        let x_half = oval_half_width(0.0); // sqrt(3)*PI/2
        MapFrame {
            dots_w,
            dots_h,
            lambda0: lambda0_deg.to_radians(),
            x_min: -x_half,
            x_max: x_half,
            y_min: -PI / 2.0,
            y_max: PI / 2.0,
        }
    }

    /// Uniform dots-per-map-unit scale: the bbox fits inside the dot grid.
    /// `pub(crate)`: the raster scanline must use the identical mapping so land and
    /// shading/terminator layers stay registered to the dot.
    pub(crate) fn scale(&self) -> f64 {
        debug_assert!(
            self.x_max > self.x_min
                && self.y_max > self.y_min
                && self.dots_w > 0
                && self.dots_h > 0,
            "MapFrame must be well-formed (build it with MapFrame::new)"
        );
        let sx = self.dots_w as f64 / (self.x_max - self.x_min);
        let sy = self.dots_h as f64 / (self.y_max - self.y_min);
        if sx < sy {
            sx
        } else {
            sy
        }
    }

    /// Map-space (x, y) of a dot's center; dy = 0 is the top (north) row.
    /// `pub(crate)` for the same registration reason as [`Self::scale`].
    pub(crate) fn dot_center(&self, dx: usize, dy: usize) -> (f64, f64) {
        let s = self.scale();
        let x = (dx as f64 + 0.5 - self.dots_w as f64 / 2.0) / s;
        let y = (self.dots_h as f64 / 2.0 - (dy as f64 + 0.5)) / s;
        (x, y)
    }

    /// Dot (dx, dy), dy=0 = top row (north). None if outside the map oval
    /// (|x| > 1.5·π·sqrt(1/3 - (lat/π)²), or |lat| > π/2, or off the grid).
    /// Membership carries a tiny EPS tolerance: dot centers exactly on the
    /// boundary count as inside. Returns absolute (lon, lat) radians:
    /// lon = lon_offset + lambda0 in (-pi, pi].
    pub fn dot_to_lonlat(&self, dx: usize, dy: usize) -> Option<(f64, f64)> {
        if self.dots_w == 0 || self.dots_h == 0 || dx >= self.dots_w || dy >= self.dots_h {
            return None;
        }
        let (x, y) = self.dot_center(dx, dy);
        if y.abs() > PI / 2.0 + EPS || x.abs() > oval_half_width(y) + EPS {
            return None; // dot center outside the map oval
        }
        let (lon_rel, _) = inverse(x, y);
        Some((normalize_lon(self.lambda0 + lon_rel), y))
    }

    /// Nearest dot for (lon_deg, lat_deg) degrees in. None if outside the oval
    /// (invalid latitude, or the nearest dot's center lies off the oval — with
    /// the same EPS boundary tolerance as [`Self::dot_to_lonlat`]).
    pub fn lonlat_to_dot(&self, lon_deg: f64, lat_deg: f64) -> Option<(usize, usize)> {
        if self.dots_w == 0 || self.dots_h == 0 || !lon_deg.is_finite() || !lat_deg.is_finite() {
            return None;
        }
        let lat = lat_deg.to_radians();
        if lat.abs() > PI / 2.0 + EPS {
            return None; // invalid latitude (beyond the poles)
        }
        let (x, y) = forward(lon_deg.to_radians(), lat, self.lambda0);
        if y.abs() > PI / 2.0 + EPS || x.abs() > oval_half_width(y) + EPS {
            return None;
        }
        let s = self.scale();
        let fx = x * s + self.dots_w as f64 / 2.0 - 0.5;
        let fy = self.dots_h as f64 / 2.0 - y * s - 0.5;
        let dx = fx.round().clamp(0.0, (self.dots_w - 1) as f64) as usize;
        let dy = fy.round().clamp(0.0, (self.dots_h - 1) as f64) as usize;
        self.dot_to_lonlat(dx, dy)?;
        Some((dx, dy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqrt3() -> f64 {
        3.0_f64.sqrt()
    }

    /// |x| at lon = ±pi, lat = 0: the equator half-width sqrt(3)·pi/2.
    fn equator_half_width() -> f64 {
        sqrt3() * PI / 2.0
    }

    /// Smallest signed difference a - b modulo 2·pi, in (-pi, pi].
    fn ang_diff(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(2.0 * PI);
        if d > PI {
            d - 2.0 * PI
        } else {
            d
        }
    }

    // ------------------------------------------------------------------ projection

    #[test]
    fn normalize_lon_exact_cases() {
        assert_eq!(normalize_lon(0.0), 0.0);
        assert!((normalize_lon(0.75) - 0.75).abs() < 1e-12);
        assert!((normalize_lon(-0.75) + 0.75).abs() < 1e-12);
        assert!((normalize_lon(PI / 2.0) - PI / 2.0).abs() < 1e-12);
        // Range is (-pi, pi]: both boundaries land on +pi.
        assert_eq!(normalize_lon(PI), PI);
        assert_eq!(normalize_lon(-PI), PI);
        assert!((normalize_lon(PI + 0.5) - (-PI + 0.5)).abs() < 1e-12);
        assert!((normalize_lon(-PI - 0.5) - (PI - 0.5)).abs() < 1e-12);
        assert!((normalize_lon(2.0 * PI + 0.25) - 0.25).abs() < 1e-12);
        assert!((normalize_lon(-2.0 * PI - 0.25) + 0.25).abs() < 1e-12);
        assert!((normalize_lon(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-12);
        assert!((normalize_lon(-3.0 * PI / 2.0) - PI / 2.0).abs() < 1e-12);
    }

    #[test]
    fn normalize_lon_range_and_congruence() {
        let samples = [
            100.0,
            -100.0,
            123.456,
            -987.654,
            10.0 * PI - 0.1,
            -10.0 * PI + 0.1,
            1e6,
            -1e6 + 0.5,
        ];
        for v in samples {
            let r = normalize_lon(v);
            assert!(r > -PI && r <= PI, "range violated for {v}: {r}");
            assert!(
                ang_diff(r, v).abs() < 1e-9,
                "congruence violated for {v}: {r}"
            );
        }
    }

    #[test]
    fn equator_half_width_reference() {
        let (x, y) = forward(PI, 0.0, 0.0);
        assert!((x - equator_half_width()).abs() < 1e-12, "x = {x}");
        assert_eq!(y, 0.0);
    }

    #[test]
    fn pole_line_half_of_equator() {
        let eq = forward(PI, 0.0, 0.0).0;
        let north = forward(PI, PI / 2.0, 0.0).0;
        let south = forward(PI, -PI / 2.0, 0.0).0;
        assert!((north - eq / 2.0).abs() < 1e-12);
        assert!((south - eq / 2.0).abs() < 1e-12);
        assert!((north - sqrt3() * PI / 4.0).abs() < 1e-12);
    }

    #[test]
    fn x_at_quarter_lon_reference() {
        let (x, _) = forward(PI / 2.0, 0.0, 0.0);
        assert!((x - sqrt3() * PI / 4.0).abs() < 1e-12);
        assert!((x - 1.36035).abs() < 1e-5, "x = {x}");
    }

    #[test]
    fn y_equals_latitude() {
        for lat in [
            0.0,
            1.0,
            -1.0,
            PI / 6.0,
            -PI / 4.0,
            PI / 2.0,
            -PI / 2.0,
            -1.8, // beyond the poles, still inside the sqrt domain (~±103.9°)
        ] {
            let (_, y) = forward(0.7, lat, 0.3);
            assert_eq!(y, lat);
        }
    }

    #[test]
    fn forward_rotates_by_central_meridian() {
        let l0 = PI / 6.0;
        let a = forward(l0 + PI / 4.0, 0.3, l0);
        let b = forward(PI / 4.0, 0.3, 0.0);
        assert!((a.0 - b.0).abs() < 1e-12);
        assert!((a.1 - b.1).abs() < 1e-12);
        // Adding whole turns of longitude changes nothing.
        let c = forward(l0 + PI / 4.0 + 4.0 * PI, 0.3, l0);
        assert!((c.0 - b.0).abs() < 1e-12);
        // Central meridian on the dateline: -pi/2 absolute is +pi/2 relative.
        let d = forward(-PI / 2.0, 0.2, PI);
        let e = forward(PI / 2.0, 0.2, 0.0);
        assert!((d.0 - e.0).abs() < 1e-12);
    }

    #[test]
    fn inverse_reference_values() {
        let (lon, lat) = inverse(equator_half_width(), 0.0);
        assert!(ang_diff(lon, PI).abs() < 1e-9, "lon = {lon}");
        assert_eq!(lat, 0.0);

        let (lon, lat) = inverse(0.0, PI / 2.0);
        assert!(lon.abs() < 1e-12);
        assert_eq!(lat, PI / 2.0);

        // The -pi edge of the range maps to +pi.
        let (lon, _) = inverse(-equator_half_width(), 0.0);
        assert!(ang_diff(lon, PI).abs() < 1e-9, "lon = {lon}");

        let (lon, lat) = inverse(1.0, 0.5);
        let expect = 2.0 / (3.0 * (1.0 / 3.0 - (0.5 / PI) * (0.5 / PI)).sqrt());
        assert!((lon - expect).abs() < 1e-12);
        assert_eq!(lat, 0.5);
    }

    #[test]
    fn round_trip_lonlat_via_xy() {
        let lats = [
            0.0,
            1.0,
            -1.0,
            PI / 6.0,
            -PI / 4.0,
            PI / 2.0,
            -PI / 2.0,
            PI / 2.0 + 1e-3,
            -PI / 2.0 - 1e-3,
        ];
        let lons = [
            0.0,
            PI / 6.0,
            -PI / 6.0,
            PI / 4.0,
            PI / 2.0,
            -PI / 2.0,
            3.0 * PI / 4.0,
            PI,
            -PI,
            2.0,
            -2.0,
            PI - 1e-9,
        ];
        let l0s = [0.0, PI / 6.0, -2.0, PI];
        for &l0 in &l0s {
            for &lat in &lats {
                for &lon in &lons {
                    let (x, y) = forward(lon, lat, l0);
                    let (lon2, lat2) = inverse(x, y);
                    let want = normalize_lon(lon - l0);
                    assert!(
                        ang_diff(lon2, want).abs() < 1e-9,
                        "lon={lon} lat={lat} l0={l0}: {lon2} vs {want}"
                    );
                    assert!((lat2 - lat).abs() < 1e-9, "lat={lat} l0={l0}: {lat2}");
                }
            }
        }
    }

    #[test]
    fn round_trip_xy_via_lonlat() {
        let lats = [0.0, 0.5, -0.5, 1.0, -1.0, PI / 2.0, -PI / 2.0];
        let fracs = [-0.99, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 0.99];
        for &lat in &lats {
            for &f in &fracs {
                let x = f * oval_half_width(lat);
                let (lon, lat2) = inverse(x, lat);
                let (x2, y2) = forward(lon, lat2, 0.0);
                assert!((x2 - x).abs() < 1e-9, "lat={lat} f={f}: {x2} vs {x}");
                assert!((y2 - lat).abs() < 1e-9, "lat={lat} f={f}: {y2}");
            }
        }
    }

    #[test]
    fn symmetry() {
        let lons = [0.3, 1.0, 2.5, PI / 2.0, -0.7, -2.9]; // strictly inside (-pi, pi)
        let lats = [0.0, 0.4, -0.4, 1.2, -1.2, PI / 2.0, -PI / 2.0];
        for &lon in &lons {
            for &lat in &lats {
                let (xp, yp) = forward(lon, lat, 0.0);
                let (xn, _) = forward(-lon, lat, 0.0);
                assert!((xn + xp).abs() < 1e-12, "x odd in lon: {lon} {lat}");
                let (xs, ys) = forward(lon, -lat, 0.0);
                assert!((xs - xp).abs() < 1e-12, "x even in lat: {lon} {lat}");
                assert!((ys + yp).abs() < 1e-12, "y odd in lat: {lon} {lat}");
            }
        }
    }

    // ------------------------------------------------------------------ MapFrame

    #[test]
    fn frame_bbox_fields() {
        let f = MapFrame::new(200, 60, 30.0);
        assert_eq!(f.dots_w, 200);
        assert_eq!(f.dots_h, 60);
        assert!((f.lambda0 - 30.0_f64.to_radians()).abs() < 1e-12);
        assert!((f.x_min + equator_half_width()).abs() < 1e-12);
        assert!((f.x_max - equator_half_width()).abs() < 1e-12);
        assert_eq!(f.y_min, -PI / 2.0);
        assert_eq!(f.y_max, PI / 2.0);
        // The bbox ties back to the projection's extreme.
        assert!((f.x_max - forward(PI, 0.0, 0.0).0).abs() < 1e-12);
    }

    #[test]
    fn frame_degenerate_sizes_do_not_panic() {
        // 1x1: the single dot sits exactly at the map center.
        let f1 = MapFrame::new(1, 1, 0.0);
        assert_eq!(f1.dot_to_lonlat(0, 0), Some((0.0, 0.0)));
        assert_eq!(f1.dot_to_lonlat(1, 0), None);
        assert_eq!(f1.lonlat_to_dot(10.0, 20.0), Some((0, 0)));

        // 200x60 (extreme aspect): iterating the whole grid is safe and non-empty.
        let f2 = MapFrame::new(200, 60, -45.0);
        let mut valid = 0;
        for dy in 0..f2.dots_h {
            for dx in 0..f2.dots_w {
                if f2.dot_to_lonlat(dx, dy).is_some() {
                    valid += 1;
                }
            }
        }
        assert!(valid > 0, "some dots must be inside the oval");

        // Zero-sized grids: no dot exists, nothing panics.
        for (w, h) in [(0usize, 0usize), (0, 5), (5, 0)] {
            let f = MapFrame::new(w, h, 0.0);
            assert_eq!(f.dot_to_lonlat(0, 0), None);
            assert_eq!(f.lonlat_to_dot(0.0, 0.0), None);
        }
    }

    #[test]
    fn frame_top_row_is_north() {
        let f = MapFrame::new(101, 41, 0.0); // odd sizes -> exact center dot (50, 20)
        let (lon, lat) = f.dot_to_lonlat(50, 20).expect("center dot inside oval");
        assert!(lon.abs() < 1e-12 && lat.abs() < 1e-12);
        let (_, lat_top) = f.dot_to_lonlat(50, 0).expect("top-center inside");
        let (_, lat_bottom) = f.dot_to_lonlat(50, 40).expect("bottom-center inside");
        assert!(lat_top > 0.0, "dy=0 must be the north row");
        assert!(lat_bottom < 0.0);
        assert!((lat_top + lat_bottom).abs() < 1e-12);
    }

    #[test]
    fn frame_dot_grid_matches_uniform_scale() {
        // 200x60 is height-limited: uniform scale = 60/pi dots per map unit.
        let f = MapFrame::new(200, 60, 0.0);
        let s = 60.0 / PI;
        for dy in [0usize, 1, 30, 59] {
            let (_, lat) = f.dot_to_lonlat(100, dy).expect("center column inside");
            let want = (30.0 - dy as f64 - 0.5) * PI / 60.0;
            assert!((lat - want).abs() < 1e-12, "dy={dy}: {lat} vs {want}");
        }
        // Vertical spacing: one dot = 1/s map units.
        let l30 = f.dot_to_lonlat(100, 30).unwrap().1;
        let l31 = f.dot_to_lonlat(100, 31).unwrap().1;
        assert!((l30 - l31 - 1.0 / s).abs() < 1e-12);
        // Horizontal spacing is the same 1/s (uniform scale, ~square dots).
        let y = l30;
        let k = 1.5 * (1.0 / 3.0 - (y / PI) * (y / PI)).sqrt(); // dx/dlon at this row
        for dx in [100usize, 101, 102] {
            let (lon, _) = f.dot_to_lonlat(dx, 30).expect("inside");
            let want_x = (dx as f64 + 0.5 - 100.0) / s;
            assert!((lon * k - want_x).abs() < 1e-12, "dx={dx}");
        }
    }

    #[test]
    fn frame_round_trip_all_dots() {
        let f = MapFrame::new(200, 60, 30.0);
        let mut valid = 0;
        for dy in 0..f.dots_h {
            for dx in 0..f.dots_w {
                if let Some((lon, lat)) = f.dot_to_lonlat(dx, dy) {
                    valid += 1;
                    let back = f.lonlat_to_dot(lon.to_degrees(), lat.to_degrees());
                    assert_eq!(
                        back,
                        Some((dx, dy)),
                        "dot ({dx},{dy}) -> ({lon},{lat}) -> {back:?}"
                    );
                }
            }
        }
        // Sanity band: the oval covers ~85% of its bbox (~104x60 dots at this scale).
        assert!((5000..=5600).contains(&valid), "valid dots = {valid}");
    }

    #[test]
    fn frame_none_outside_oval() {
        let f = MapFrame::new(200, 60, 0.0);
        for (dx, dy) in [(0, 0), (199, 0), (0, 59), (199, 59), (10, 0), (10, 59)] {
            assert!(
                f.dot_to_lonlat(dx, dy).is_none(),
                "corner ({dx},{dy}) must be outside the oval"
            );
        }
        // Top-center (pole line) is inside.
        assert!(f.dot_to_lonlat(100, 0).is_some());
        // Off-grid coordinates.
        assert_eq!(f.dot_to_lonlat(200, 30), None);
        assert_eq!(f.dot_to_lonlat(100, 60), None);
    }

    #[test]
    fn frame_poles_map_to_top_and_bottom_rows() {
        let f = MapFrame::new(200, 60, 0.0);
        assert_eq!(f.lonlat_to_dot(0.0, 90.0), Some((100, 0)));
        assert_eq!(f.lonlat_to_dot(0.0, -90.0), Some((100, 59)));
    }

    #[test]
    fn frame_none_when_nearest_dot_off_oval() {
        // 8x8 is width-limited: the two topmost and bottommost dot rows lie beyond
        // the poles, so no dot can represent the pole itself.
        let f = MapFrame::new(8, 8, 0.0);
        for dx in 0..8 {
            for dy in [0usize, 1, 6, 7] {
                assert_eq!(f.dot_to_lonlat(dx, dy), None, "({dx},{dy})");
            }
        }
        assert!(f.dot_to_lonlat(4, 3).is_some());
        assert_eq!(f.lonlat_to_dot(0.0, 90.0), None);
        assert_eq!(f.lonlat_to_dot(0.0, -90.0), None);
        // Invalid latitudes outright.
        assert_eq!(f.lonlat_to_dot(0.0, 95.0), None);
        assert_eq!(f.lonlat_to_dot(0.0, -95.0), None);
    }

    #[test]
    fn frame_nearest_dot_selection() {
        // Odd grid: dot (100, 30) sits exactly at map (0, 0) — equator, central meridian.
        let f = MapFrame::new(201, 61, 0.0);
        assert_eq!(f.dot_to_lonlat(100, 30), Some((0.0, 0.0)));
        let pitch_lon = f.dot_to_lonlat(101, 30).unwrap().0; // one dot east, equator row
        let pitch_lat = f.dot_to_lonlat(100, 30).unwrap().1 - f.dot_to_lonlat(100, 31).unwrap().1;
        assert!(pitch_lon > 0.0, "east must be +x");
        assert!(pitch_lat > 0.0, "north must be up (-dy)");
        // Within half a dot pitch of a center, that dot is the nearest.
        assert_eq!(
            f.lonlat_to_dot((0.4 * pitch_lon).to_degrees(), 0.0),
            Some((100, 30))
        );
        assert_eq!(
            f.lonlat_to_dot((0.6 * pitch_lon).to_degrees(), 0.0),
            Some((101, 30))
        );
        assert_eq!(
            f.lonlat_to_dot((-0.6 * pitch_lon).to_degrees(), 0.0),
            Some((99, 30))
        );
        assert_eq!(
            f.lonlat_to_dot(0.0, (0.4 * pitch_lat).to_degrees()),
            Some((100, 30))
        );
        assert_eq!(
            f.lonlat_to_dot(0.0, (0.6 * pitch_lat).to_degrees()),
            Some((100, 29))
        );
        assert_eq!(
            f.lonlat_to_dot(0.0, (-0.6 * pitch_lat).to_degrees()),
            Some((100, 31))
        );
    }

    #[test]
    fn frame_dot_to_lonlat_applies_lambda0() {
        for (l0_deg, want_lon) in [
            (0.0, 0.0),
            (90.0, PI / 2.0),
            (180.0, PI),
            (-170.0, -170.0_f64.to_radians()),
        ] {
            let f = MapFrame::new(101, 41, l0_deg);
            let (lon, lat) = f.dot_to_lonlat(50, 20).expect("center dot inside");
            assert!((lon - want_lon).abs() < 1e-12, "l0={l0_deg}: lon={lon}");
            assert!(lat.abs() < 1e-12);
        }
        // One dot east of center on a 90-degree frame: absolute lon = pi/2 + offset.
        let f = MapFrame::new(101, 41, 90.0);
        let s = 41.0 / PI; // height-limited
        let (lon, lat) = f.dot_to_lonlat(51, 20).unwrap();
        let want = PI / 2.0 + 2.0 / (3.0 * (1.0_f64 / 3.0).sqrt() * s);
        assert!((lon - want).abs() < 1e-12, "lon={lon} want={want}");
        assert!(lat.abs() < 1e-12);
    }
}
