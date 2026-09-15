//! NOAA low-precision solar position. See INTERFACES.md and PLAN.md §6.2.
//!
//! Formulas follow the NOAA "General Solar Position Calculations"
//! (gml.noaa.gov) low-precision Fourier series in the fractional year —
//! amply accurate for a terminal daylight map across ~1950–2050.
//!
//! Units: **degrees** at this module's public boundary (per INTERFACES.md);
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
/// γ = 2π/N · (day_of_year − 1 + (UTC_hours − 12)/24), N = 366 in leap years,
/// else 365.
fn fractional_year(t: DateTime<Utc>) -> f64 {
    let y = t.year();
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let n = if leap { 366.0 } else { 365.0 };
    std::f64::consts::TAU / n * (t.ordinal() as f64 - 1.0 + (utc_decimal_hours(t) - 12.0) / 24.0)
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
/// polar day/night band for that elevation (|cos H| > 1, or the degenerate
/// pole case where cos φ = 0).
pub fn curve_lons(lat_deg: f64, sun: &Sun, alpha_deg: f64) -> Option<(f64, f64)> {
    let lat = lat_deg * DEG2RAD;
    let decl = sun.decl_deg * DEG2RAD;
    let cos_h = ((alpha_deg * DEG2RAD).sin() - lat.sin() * decl.sin()) / (lat.cos() * decl.cos());
    if !(-1.0..=1.0).contains(&cos_h) {
        return None; // polar day or polar night at this latitude (NaN also lands here)
    }
    let h = cos_h.acos() * RAD2DEG;
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
pub fn sample_curve(sun: &Sun, alpha_deg: f64, step_deg: f64) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    if step_deg <= 0.0 {
        return pts;
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
    pts
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
        // Near equinox there are no polar bands: every row −88..88 contributes.
        let sun = subsolar(utc(2024, 3, 20, 12, 0, 0));
        let pts = sample_curve(&sun, 0.0, 2.0);
        assert_eq!(pts.len(), 89 * 2, "expected every latitude row");
        for &(lat, lon) in &pts {
            assert!(
                elevation_deg(lat, lon, &sun).abs() <= 0.5,
                "point ({lat},{lon}) off the curve"
            );
        }
    }

    #[test]
    fn sample_curve_degenerate_steps() {
        let sun = subsolar(june_solstice());
        assert!(sample_curve(&sun, 0.0, 0.0).is_empty());
        assert!(sample_curve(&sun, 0.0, -2.0).is_empty());
        // Step of 90°: only the equator row, its two branches.
        let pts = sample_curve(&sun, 0.0, 90.0);
        assert_eq!(pts.len(), 2);
        let (lat, lon) = pts[0];
        assert_eq!(lat, 0.0);
        assert!(elevation_deg(lat, lon, &sun).abs() <= 0.5);
    }
}
