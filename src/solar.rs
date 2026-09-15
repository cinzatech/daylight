//! NOAA low-precision solar position. See PLAN.md §6.2.
//!
//! Formulas follow the NOAA "General Solar Position Calculations"
//! (gml.noaa.gov) low-precision Fourier series in the fractional year —
//! amply accurate for a terminal daylight map across ~1950–2050. Series
//! accuracy: about ±0.5° in declination (the Spencer series' native 0.01
//! *radian* error) and ≲1 min in the equation of time — both below one dot
//! row / a couple of minutes of terminator motion at map resolutions.
//!
//! Units: **degrees** at this module's public boundary;
//! radians only inside trig calls. All functions are pure.
//!
//! Note on the subsolar longitude: the subsolar meridian is where local
//! *apparent* solar time is noon, i.e. λs = 15°·(12 − UTC_hours − eqtime/60)
//! = −15°·(UTC_hours − 12 + eqtime/60), normalized to (−180, 180]. The
//! `UTC_hours − 12` offset is what puts λs ≈ 0 at 12:00 UTC and λs ≈ −90°
//! at 18:00 UTC.

use chrono::{DateTime, Datelike, Timelike, Utc};

const DEG2RAD: f64 = std::f64::consts::PI / 180.0;
const RAD2DEG: f64 = 180.0 / std::f64::consts::PI;

/// Subsolar point: where the Sun is at zenith (declination, longitude east).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    pub decl_deg: f64,
    pub lon_deg: f64,
}

/// Sun-elevation shading class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shading {
    Day,
    CivilTwilight,
    Night,
}

/// UTC time of day in decimal hours, 0..24 (second resolution).
fn utc_decimal_hours(t: DateTime<Utc>) -> f64 {
    t.hour() as f64 + t.minute() as f64 / 60.0 + t.second() as f64 / 3600.0
}

/// Fractional-year angle γ in radians:
/// γ = 2π/365 · (day_of_year − 1 + (UTC_hours − 12)/24). The divisor is 365
/// unconditionally, exactly as the NOAA series is calibrated: using 366 in
/// leap years re-phases the series and measurably *worsens* the fit
/// (≈0.5° vs ≈0.3° max declination error in leap years).
fn fractional_year(t: DateTime<Utc>) -> f64 {
    std::f64::consts::TAU / 365.0
        * (t.ordinal() as f64 - 1.0 + (utc_decimal_hours(t) - 12.0) / 24.0)
}

/// Normalize any longitude in degrees to (−180, 180].
fn normalize_lon_deg(lon: f64) -> f64 {
    let x = lon.rem_euclid(360.0); // [0, 360)
    if x > 180.0 {
        x - 360.0
    } else {
        x
    }
}

/// Subsolar point at instant `t`: lat = declination,
/// lon = −15°·(UTC_hours − 12 + eqtime/60), normalized to (−180, 180].
pub fn subsolar(t: DateTime<Utc>) -> Sun {
    let eqt = eq_time_minutes(t);
    let lon = normalize_lon_deg(-15.0 * (utc_decimal_hours(t) - 12.0 + eqt / 60.0));
    Sun {
        decl_deg: declination_deg(t),
        lon_deg: lon,
    }
}

/// Solar declination in degrees (positive north), from the NOAA series.
pub fn declination_deg(t: DateTime<Utc>) -> f64 {
    let g = fractional_year(t);
    let decl_rad = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    decl_rad * RAD2DEG
}

/// Equation of time in minutes (apparent − mean solar time; + means Sun ahead
/// of mean time).
pub fn eq_time_minutes(t: DateTime<Utc>) -> f64 {
    let g = fractional_year(t);
    229.18
        * (0.000075 + 0.001868 * g.cos()
            - 0.032077 * g.sin()
            - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin())
}

/// Sun elevation above the horizon at (lat_deg, lon_deg), degrees.
///
/// sin α = sin φ sin δ + cos φ cos δ cos H with hour angle
/// H = lon − subsolar lon (all degrees). sin α is clamped to [−1, 1] before
/// asin so zenith/nadir evaluate to exactly ±90°.
pub fn elevation_deg(lat_deg: f64, lon_deg: f64, sun: &Sun) -> f64 {
    let lat = lat_deg * DEG2RAD;
    let decl = sun.decl_deg * DEG2RAD;
    let h = (lon_deg - sun.lon_deg) * DEG2RAD;
    let sin_alpha = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    sin_alpha.clamp(-1.0, 1.0).asin() * RAD2DEG
}

/// Classify an elevation: ≥ 0° day, −6°..0° civil twilight, < −6° night.
pub fn shading(elev_deg: f64) -> Shading {
    if elev_deg >= 0.0 {
        Shading::Day
    } else if elev_deg >= -6.0 {
        Shading::CivilTwilight
    } else {
        Shading::Night
    }
}

/// Longitude pair (degrees east) where the Sun's elevation equals `alpha_deg`
/// at latitude `lat_deg`: (subsolar_lon + H, subsolar_lon − H), each
/// normalized to (−180, 180]. Returns `None` when the latitude lies in the
/// polar day/night band for that elevation (|cos H| > 1 beyond a 1e-12
/// tolerance, so NaN and genuinely polar latitudes land here, while the
/// exact tangent row — whose floating-point cos H can sit a few ulps outside
/// [−1, 1] — is admitted and returns its merged branch pair). The degenerate
/// pole case (cos φ = 0) also lands in `None`.
pub fn curve_lons(lat_deg: f64, sun: &Sun, alpha_deg: f64) -> Option<(f64, f64)> {
    const COS_H_TOL: f64 = 1e-12;
    let lat = lat_deg * DEG2RAD;
    let decl = sun.decl_deg * DEG2RAD;
    let cos_h = ((alpha_deg * DEG2RAD).sin() - lat.sin() * decl.sin()) / (lat.cos() * decl.cos());
    if !(-1.0 - COS_H_TOL..=1.0 + COS_H_TOL).contains(&cos_h) {
        return None; // polar day or polar night at this latitude (NaN also lands here)
    }
    let h = cos_h.clamp(-1.0, 1.0).acos() * RAD2DEG;
    Some((
        normalize_lon_deg(sun.lon_deg + h),
        normalize_lon_deg(sun.lon_deg - h),
    ))
}

/// Densely sample the elevation == `alpha_deg` curve: for latitudes from
/// −90°+`step_deg` to 90°−`step_deg` inclusive (count-indexed to avoid float
/// accumulation drift), push both branch points per latitude. Latitudes
/// inside polar bands are skipped. Points come in per-latitude pairs
/// (same lat, the two branch longitudes).
///
/// The curve's poleward tangent tips (where the two branches merge) generically
/// fall *between* sampled rows — and near a tangent the curve closes like √ε,
/// so the last sampled row leaves a visible longitude gap. Each valid tangent
/// latitude is therefore also emitted as a merged pair (unless a sampled row
/// already sits exactly on it).
pub fn sample_curve(sun: &Sun, alpha_deg: f64, step_deg: f64) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    // NaN must be rejected too (a bare `step_deg <= 0.0` test is false for NaN,
    // which would loop forever below).
    if step_deg.is_nan() || step_deg <= 0.0 {
        return pts; // non-positive or NaN: degenerate, nothing to sample
    }
    let mut i: usize = 1;
    loop {
        let lat = -90.0 + i as f64 * step_deg;
        if lat > 90.0 - step_deg + 1e-9 {
            break;
        }
        if let Some((lon_a, lon_b)) = curve_lons(lat, sun, alpha_deg) {
            pts.push((lat, lon_a));
            pts.push((lat, lon_b));
        }
        i += 1;
    }
    for tangent_lat in tangent_lats(sun, alpha_deg) {
        // Skip tangents that a sampled row already covers exactly (its merged
        // pair was pushed by the loop above).
        let row = (tangent_lat + 90.0) / step_deg;
        let on_sampled_row = row >= 1.0 && row.fract().abs() < 1e-9;
        if !on_sampled_row {
            if let Some((lon_a, lon_b)) = curve_lons(tangent_lat, sun, alpha_deg) {
                pts.push((tangent_lat, lon_a));
                pts.push((tangent_lat, lon_b));
            }
        }
    }
    pts
}

/// Latitudes where the elevation == α curve turns around (cos H = ±1, the
/// two branches merging): φ ∈ {δ + 90 − α, δ − 90 + α, 90 + α − δ, −90 − α − δ},
/// kept inside the open interval (−90°, 90°) and deduplicated. At α = 0 this
/// is the classic ±(90° − |δ|) pair; the poles themselves (δ = ±α degenerate
/// case) are excluded.
fn tangent_lats(sun: &Sun, alpha_deg: f64) -> Vec<f64> {
    let d = sun.decl_deg;
    let a = alpha_deg;
    let mut out: Vec<f64> = Vec::new();
    for t in [d + 90.0 - a, d - 90.0 + a, 90.0 + a - d, -90.0 - a - d] {
        if t > -90.0 + 1e-9 && t < 90.0 - 1e-9 && !out.iter().any(|&e| (e - t).abs() < 1e-9) {
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap()
    }
    fn june_solstice() -> DateTime<Utc> {
        utc(2000, 6, 21, 12, 0, 0)
    }
    fn dec_solstice() -> DateTime<Utc> {
        utc(2000, 12, 21, 12, 0, 0)
    }
    fn march_equinox() -> DateTime<Utc> {
        utc(2000, 3, 20, 12, 0, 0)
    }

    // ---- declination -----------------------------------------------------

    #[test]
    fn declination_at_june_solstice() {
        let d = declination_deg(june_solstice());
        assert!((d - 23.44).abs() <= 0.3, "2000 June decl {d}");
        // Same date in a non-leap year exercises the 365-day divisor.
        let d = declination_deg(utc(2023, 6, 21, 12, 0, 0));
        assert!((d - 23.44).abs() <= 0.3, "2023 June decl {d}");
    }

    #[test]
    fn declination_at_december_solstice() {
        let d = declination_deg(dec_solstice());
        assert!((d + 23.44).abs() <= 0.3, "2000 December decl {d}");
        let d = declination_deg(utc(2023, 12, 21, 12, 0, 0));
        assert!((d + 23.44).abs() <= 0.3, "2023 December decl {d}");
    }

    #[test]
    fn declination_near_zero_at_march_equinox() {
        let d = declination_deg(march_equinox());
        assert!(d.abs() <= 0.5, "March equinox decl {d}");
    }

    // ---- equation of time -------------------------------------------------

    #[test]
    fn equation_of_time_extremes() {
        let feb = eq_time_minutes(utc(2000, 2, 11, 12, 0, 0));
        assert!((feb + 14.2).abs() <= 1.5, "February eqtime {feb}");
        let nov = eq_time_minutes(utc(2000, 11, 3, 12, 0, 0));
        assert!((nov - 16.4).abs() <= 1.5, "November eqtime {nov}");
    }

    // ---- subsolar point ----------------------------------------------------

    #[test]
    fn subsolar_longitude_tracks_utc_hours() {
        // Noon UTC: Sun near the Greenwich meridian.
        let noon = subsolar(june_solstice());
        assert!(noon.lon_deg.abs() <= 4.0, "June noon lon {}", noon.lon_deg);
        let noon_d = subsolar(dec_solstice());
        assert!(
            noon_d.lon_deg.abs() <= 4.0,
            "Dec noon lon {}",
            noon_d.lon_deg
        );
        // 18:00 UTC: Sun ~90° west.
        let evening = subsolar(utc(2000, 6, 21, 18, 0, 0));
        assert!(
            (evening.lon_deg + 90.0).abs() <= 4.0,
            "June 18h lon {}",
            evening.lon_deg
        );
        let evening_d = subsolar(utc(2000, 12, 21, 18, 0, 0));
        assert!(
            (evening_d.lon_deg + 90.0).abs() <= 4.0,
            "Dec 18h lon {}",
            evening_d.lon_deg
        );
    }

    #[test]
    fn subsolar_point_stays_in_domain() {
        for h in 0u32..24 {
            let s = subsolar(utc(2000, 6, 21, h, 0, 0));
            assert!(
                -180.0 < s.lon_deg && s.lon_deg <= 180.0,
                "lon out of (-180,180]: {}",
                s.lon_deg
            );
            assert!(
                s.decl_deg.abs() <= 24.5,
                "decl out of range: {}",
                s.decl_deg
            );
        }
        // Across the dateline, e.g. 23:00 UTC is ~165°W (the Sun moves west).
        let late = subsolar(utc(2000, 6, 21, 23, 0, 0));
        assert!(late.lon_deg < -160.0, "23h lon {}", late.lon_deg);
    }

    // ---- elevation ----------------------------------------------------------

    #[test]
    fn sun_is_at_zenith_over_subsolar_point() {
        for ts in [
            june_solstice(),
            dec_solstice(),
            march_equinox(),
            utc(2024, 8, 9, 6, 30, 0),
        ] {
            let sun = subsolar(ts);
            let e = elevation_deg(sun.decl_deg, sun.lon_deg, &sun);
            assert!((e - 90.0).abs() <= 1.5, "zenith elev {e} at {ts}");
        }
    }

    #[test]
    fn sun_is_at_nadir_over_the_antipode() {
        for ts in [june_solstice(), march_equinox()] {
            let sun = subsolar(ts);
            let anti_lat = -sun.decl_deg;
            let anti_lon = normalize_lon_deg(sun.lon_deg + 180.0);
            let e = elevation_deg(anti_lat, anti_lon, &sun);
            assert!((e + 90.0).abs() <= 1.5, "nadir elev {e} at {ts}");
        }
    }

    #[test]
    fn elevation_sign_around_subsolar_meridian() {
        let sun = subsolar(march_equinox());
        assert!(elevation_deg(0.0, sun.lon_deg, &sun) > 0.9);
        assert!(elevation_deg(0.0, sun.lon_deg + 180.0, &sun) < -0.9);
        assert!(elevation_deg(0.0, sun.lon_deg + 45.0, &sun) > 0.0);
        assert!(elevation_deg(0.0, sun.lon_deg + 135.0, &sun) < 0.0);
    }

    #[test]
    fn polar_night_and_polar_day_at_80n() {
        // December solstice: Sun below the horizon all day at 80°N.
        let dec_sun = subsolar(dec_solstice());
        let e = elevation_deg(80.0, 0.0, &dec_sun);
        assert!(e < 0.0, "December polar night elev {e}");
        assert_eq!(shading(e), Shading::Night);

        // June solstice: midnight Sun at 80°N.
        let jun_sun = subsolar(june_solstice());
        let e = elevation_deg(80.0, 0.0, &jun_sun);
        assert!(e > 0.0, "June polar day elev {e}");
        assert_eq!(shading(e), Shading::Day);
    }

    // ---- shading --------------------------------------------------------------

    #[test]
    fn shading_thresholds() {
        assert_eq!(shading(0.0), Shading::Day);
        assert_eq!(shading(45.2), Shading::Day);
        assert_eq!(shading(-0.001), Shading::CivilTwilight);
        assert_eq!(shading(-6.0), Shading::CivilTwilight);
        assert_eq!(shading(-6.001), Shading::Night);
        assert_eq!(shading(-40.0), Shading::Night);
    }

    // ---- terminator / twilight curves ------------------------------------------

    #[test]
    fn curve_lons_equinox_branches_are_subsolar_plus_minus_90() {
        // Real Sun at the 2024 March equinox (decl ≈ 0): the α = 0 curve is
        // the great circle 90° from the subsolar meridian.
        let sun = subsolar(utc(2024, 3, 20, 12, 0, 0));
        assert!(sun.decl_deg.abs() <= 0.5, "decl {}", sun.decl_deg);
        for lat in [0.0, 10.0, -10.0, 25.0, -25.0, 45.0, -45.0] {
            let (a, b) = curve_lons(lat, &sun, 0.0).expect("equinox curve exists at all lats");
            assert!(
                (a - (sun.lon_deg + 90.0)).abs() <= 1.5,
                "lat {lat}: east branch {a} vs {}",
                sun.lon_deg + 90.0
            );
            assert!(
                (b - (sun.lon_deg - 90.0)).abs() <= 1.5,
                "lat {lat}: west branch {b} vs {}",
                sun.lon_deg - 90.0
            );
            // Branches straddle the subsolar meridian symmetrically.
            assert!(((a + b) / 2.0 - sun.lon_deg).abs() <= 0.5);
        }
    }

    #[test]
    fn curve_lons_none_in_polar_bands_at_solstice() {
        let sun = subsolar(june_solstice()); // decl ≈ +23.46°, polar circle ≈ 66.5°
        assert!(sun.decl_deg > 23.0, "precondition decl {}", sun.decl_deg);
        for lat in [67.0, 70.0, 80.0, 89.0, -67.0, -70.0, -80.0] {
            assert!(
                curve_lons(lat, &sun, 0.0).is_none(),
                "lat {lat} should be inside a polar band"
            );
        }
        for lat in [-66.0, -45.0, 0.0, 45.0, 66.0] {
            assert!(
                curve_lons(lat, &sun, 0.0).is_some(),
                "lat {lat} should cross the terminator"
            );
        }
        // The −6° twilight curve only reaches |φ| ≲ 90 − 23.5 − 6 ≈ 60.5°.
        assert!(curve_lons(65.0, &sun, -6.0).is_none());
        assert!(curve_lons(55.0, &sun, -6.0).is_some());
    }

    #[test]
    fn curve_lons_wraps_through_the_dateline() {
        // Synthetic equinox Sun over 100°E: branches at 100±90 = 190 → −170, 10.
        let sun = Sun {
            decl_deg: 0.0,
            lon_deg: 100.0,
        };
        let (a, b) = curve_lons(0.0, &sun, 0.0).unwrap();
        assert!((a + 170.0).abs() <= 1e-9, "east branch {a}");
        assert!((b - 10.0).abs() <= 1e-9, "west branch {b}");

        let sun = Sun {
            decl_deg: 0.0,
            lon_deg: -100.0,
        };
        let (a, b) = curve_lons(0.0, &sun, 0.0).unwrap();
        assert!((a + 10.0).abs() <= 1e-9, "east branch {a}");
        assert!((b - 170.0).abs() <= 1e-9, "west branch {b}");
    }

    #[test]
    fn curve_lons_inverts_elevation() {
        // Every returned branch sits exactly on the requested elevation.
        for ts in [june_solstice(), dec_solstice(), utc(2024, 3, 20, 12, 0, 0)] {
            let sun = subsolar(ts);
            for alpha in [0.0, -6.0] {
                for lat in [-85.0, -60.0, -33.0, 0.0, 33.0, 60.0, 85.0] {
                    if let Some((a, b)) = curve_lons(lat, &sun, alpha) {
                        assert!(
                            (elevation_deg(lat, a, &sun) - alpha).abs() <= 1e-6,
                            "lat {lat} branch a off-curve"
                        );
                        assert!(
                            (elevation_deg(lat, b, &sun) - alpha).abs() <= 1e-6,
                            "lat {lat} branch b off-curve"
                        );
                    }
                }
            }
        }
    }

    // ---- curve sampling ---------------------------------------------------------

    #[test]
    fn sample_curve_points_lie_on_the_terminator() {
        let sun = subsolar(june_solstice());
        let pts = sample_curve(&sun, 0.0, 2.0);
        // |φ| ≲ 66.5° rows cross → 67 rows × 2 branches = 134 points.
        assert!(
            pts.len() > 100,
            "expected dense sampling, got {}",
            pts.len()
        );
        // Points come in per-latitude pairs.
        for ch in pts.chunks(2) {
            assert_eq!(ch[0].0, ch[1].0);
        }
        // Spot-check a spread of points: elevation within 0.5° of α = 0.
        let n = pts.len();
        for &i in &[
            0,
            1,
            n / 4,
            n / 3,
            n / 2,
            2 * n / 3,
            3 * n / 4,
            n - 2,
            n - 1,
        ] {
            let (lat, lon) = pts[i];
            let e = elevation_deg(lat, lon, &sun);
            assert!(e.abs() <= 0.5, "point {i} ({lat},{lon}) elev {e}");
        }
        for &(lat, _) in &pts {
            assert!((-90.0..90.0).contains(&lat), "lat {lat} out of range");
        }
    }

    #[test]
    fn sample_curve_points_lie_on_the_twilight_curve() {
        let sun = subsolar(june_solstice());
        let pts = sample_curve(&sun, -6.0, 2.0);
        assert!(pts.len() > 90, "expected dense sampling, got {}", pts.len());
        for &i in &[0, 5, pts.len() / 2, pts.len() - 1] {
            let (lat, lon) = pts[i];
            let e = elevation_deg(lat, lon, &sun);
            assert!((e + 6.0).abs() <= 0.5, "point {i} ({lat},{lon}) elev {e}");
        }
    }

    #[test]
    fn sample_curve_equinox_spans_all_latitudes() {
        // Near equinox there are no polar bands at row resolution: every row
        // −88..88 contributes, plus the two tangent tips just beyond the rows
        // (decl is small but nonzero at this instant, so the tangents lie in
        // (±88, ±90) and are appended as merged pairs).
        let sun = subsolar(utc(2024, 3, 20, 12, 0, 0));
        assert!(sun.decl_deg.abs() > 0.0 && sun.decl_deg.abs() < 2.0);
        let pts = sample_curve(&sun, 0.0, 2.0);
        assert_eq!(pts.len(), 89 * 2 + 4, "every row plus the two tangent tips");
        for &(lat, lon) in &pts {
            assert!(
                elevation_deg(lat, lon, &sun).abs() <= 0.5,
                "point ({lat},{lon}) off the curve"
            );
        }
    }

    #[test]
    fn curve_lons_exact_tangent_row_is_admitted() {
        // At the tangent latitude cos H is analytically −1, but floating point
        // evaluates it a few ulps outside [−1, 1] — the strict check used to
        // return None there, dropping the row that closes the curve.
        let sun = Sun {
            decl_deg: 23.44,
            lon_deg: 10.0,
        };
        let tangent = 90.0 - 23.44; // exactly 66.56°N
        let (a, b) = curve_lons(tangent, &sun, 0.0)
            .expect("exact tangent row must be admitted (within tolerance)");
        // Both branches merge on the antisolar meridian: 10° + 180° = −170°.
        assert!((a + 170.0).abs() <= 1e-6, "east branch {a}");
        assert!((b + 170.0).abs() <= 1e-6, "west branch {b}");
        // Genuinely polar latitudes stay rejected.
        assert!(curve_lons(tangent + 1.0, &sun, 0.0).is_none());
    }

    #[test]
    fn sample_curve_emits_tangent_tips() {
        // The poleward tips of the curve (where its branches merge) generically
        // fall between sampled rows; the sampler must emit them so the curve
        // closes instead of ending in a longitude gap.
        let sun = subsolar(june_solstice()); // decl ≈ +23.4x°
        let pts = sample_curve(&sun, 0.0, 2.0);
        let mut merged = 0;
        for ch in pts.chunks(2) {
            let (lat, lon_a) = ch[0];
            let lon_b = ch[1].1;
            if (lon_a - lon_b).abs() < 1e-6 {
                merged += 1;
                // The tip sits on the curve (elevation ≈ 0) at the tangent band.
                let e = elevation_deg(lat, lon_a, &sun);
                assert!(e.abs() <= 0.5, "tangent tip ({lat},{lon_a}) elev {e}");
            }
        }
        assert!(merged >= 2, "expected both poleward tips, found {merged}");
    }

    #[test]
    fn sample_curve_degenerate_steps() {
        let sun = subsolar(june_solstice());
        assert!(sample_curve(&sun, 0.0, 0.0).is_empty());
        assert!(sample_curve(&sun, 0.0, -2.0).is_empty());
        assert!(
            sample_curve(&sun, 0.0, f64::NAN).is_empty(),
            "NaN step must not loop"
        );
        // Step of 90°: the equator row's two branches plus the two tangent tips.
        let pts = sample_curve(&sun, 0.0, 90.0);
        assert_eq!(pts.len(), 6, "equator pair + 2 merged tangent pairs");
        let (lat, lon) = pts[0];
        assert_eq!(lat, 0.0);
        assert!(elevation_deg(lat, lon, &sun).abs() <= 0.5);
        // The appended tangent pairs share a latitude and a longitude.
        let (t_lat, t_a) = pts[4];
        let (t_lat2, t_b) = pts[5];
        assert_eq!(t_lat, t_lat2);
        assert!(
            (t_a - t_b).abs() < 1e-6,
            "tangent branches merge: {t_a} vs {t_b}"
        );
        assert!(
            t_lat > 60.0,
            "June tangent is in the far north, got {t_lat}"
        );
    }
}
