//! Meteor-shower association of single meteors.
//!
//! The shower list is **data, not code**: it is read at run time from a CSV in
//! the format of RMS' `established_showers.csv` (IAU working list, `|`
//! separated), so it can be updated by dropping a new file on the SD card — no
//! firmware rebuild. Lookup order:
//!
//!  1. `/media/mmc/config/showers.csv` (override)
//!  2. `/usr/share/atomet/showers.csv` (shipped in the rootfs)
//!
//! The file is re-read whenever its modification time or size changes. With no
//! usable file, association is simply skipped.
//!
//! A meteor is associated with a shower when
//!  - the shower is active at the meteor's solar longitude,
//!  - the radiant (moved by its daily drift) lies within `tol` of the
//!    meteor's great circle, and
//!  - the radiant is behind the start point (the meteor moves away from it).
//!
//! One station cannot prove membership: a random meteor also passes near a
//! given radiant by chance (probability ≈ ½·sin(offset)). The sidecar keeps
//! the offsets so a server can weigh them.

use crate::paths;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type V3 = [f64; 3];

/// Tolerance on the radiant-to-great-circle distance: `TOL_BASE` plus three
/// times the pole uncertainty a 1 px error gives on a track of this length,
/// capped (very short tracks say little about direction).
const TOL_BASE_DEG: f64 = 3.0;
const TOL_CAP_DEG: f64 = 15.0;
const POS_ERR_PX: f64 = 1.0;
/// Showers farther than this from the great circle are not listed.
const LIST_MAX_OFFSET_DEG: f64 = 30.0;
/// Activity window assumed when the list gives only the maximum.
const DEFAULT_HALF_WINDOW_DEG: f64 = 5.0;

pub struct Shower {
    pub code: String,
    pub name: String,
    /// Solar longitude (deg) of begin / maximum / end.
    pub begin: f64,
    pub max: f64,
    pub end: f64,
    /// Radiant at the maximum (deg) and drift per degree of solar longitude.
    pub ra: f64,
    pub dra: f64,
    pub dec: f64,
    pub ddec: f64,
}

/// One shower considered for a meteor.
pub struct Match {
    pub code: String,
    pub name: String,
    pub radiant_ra: f64,
    pub radiant_dec: f64,
    /// Distance from the radiant to the meteor's great circle (deg).
    pub offset_deg: f64,
    /// Radiant is behind the start point.
    pub behind: bool,
    /// `offset_deg <= tol && behind`.
    pub hit: bool,
}

pub struct Catalog {
    pub showers: Vec<Shower>,
    /// Where it came from and which version, for the sidecar.
    pub info: serde_json::Value,
}

fn dot(a: &V3, b: &V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3(ra_deg: f64, dec_deg: f64) -> V3 {
    let (ra, dec) = (ra_deg.to_radians(), dec_deg.to_radians());
    [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()]
}

fn wrap180(a: f64) -> f64 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Sun's ecliptic longitude (deg, equinox of date) at a unix time; the usual
/// low-precision series, good to ~0.01° — far finer than a list's windows.
pub fn solar_longitude(unix_s: f64) -> f64 {
    let n = unix_s / 86400.0 + 2440587.5 - 2451545.0;
    let l = 280.460 + 0.9856474 * n;
    let g = (357.528 + 0.9856003 * n).to_radians();
    (l + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).rem_euclid(360.0)
}

/// FNV-1a, to tell list versions apart in the sidecar.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Catalog {
    /// Parse the RMS `established_showers.csv` format. Lines starting with `#`
    /// and lines without a usable solar-longitude maximum are skipped.
    pub fn parse(text: &str) -> Vec<Shower> {
        let mut out = Vec::new();
        for line in text.lines() {
            if line.starts_with('#') || !line.contains('|') {
                continue;
            }
            let f: Vec<&str> = line.split('|').map(str::trim).collect();
            if f.len() < 10 {
                continue;
            }
            let num = |s: &str| s.parse::<f64>().ok();
            let (Some(max), Some(ra), Some(dec)) = (num(f[4]), num(f[6]), num(f[8])) else {
                continue;
            };
            out.push(Shower {
                code: f[1].to_string(),
                name: f[2].to_string(),
                begin: num(f[3]).unwrap_or(max - DEFAULT_HALF_WINDOW_DEG),
                max,
                end: num(f[5]).unwrap_or(max + DEFAULT_HALF_WINDOW_DEG),
                ra,
                dra: num(f[7]).unwrap_or(0.0),
                dec,
                ddec: num(f[9]).unwrap_or(0.0),
            });
        }
        out
    }

    fn from_file(path: &Path) -> Option<Catalog> {
        let bytes = std::fs::read(path).ok()?;
        let showers = Self::parse(&String::from_utf8_lossy(&bytes));
        if showers.is_empty() {
            log::warn!("Shower list {} has no usable entries", path.display());
            return None;
        }
        let info = json!({
            "source": path.display().to_string(),
            "entries": showers.len(),
            "bytes": bytes.len(),
            "fnv1a": format!("{:016x}", fnv1a(&bytes)),
        });
        log::info!("Loaded {} showers from {}", showers.len(), path.display());
        Some(Catalog { showers, info })
    }

    /// Associate a meteor going from sky point `v0` to `v1` (unit vectors,
    /// ICRS) seen at unix time `unix_s`, whose track is `length_px` long.
    /// Returns the solar longitude, the tolerance used, and every active
    /// shower whose radiant is within `LIST_MAX_OFFSET_DEG` of the great circle.
    pub fn associate(&self, v0: &V3, v1: &V3, length_px: f64, unix_s: f64) -> (f64, f64, Vec<Match>) {
        let lam = solar_longitude(unix_s);
        let tol = (TOL_BASE_DEG + 3.0 * (POS_ERR_PX / length_px.max(1.0)).to_degrees()).min(TOL_CAP_DEG);
        let n = [
            v0[1] * v1[2] - v0[2] * v1[1],
            v0[2] * v1[0] - v0[0] * v1[2],
            v0[0] * v1[1] - v0[1] * v1[0],
        ];
        let nn = dot(&n, &n).sqrt();
        let mut out = Vec::new();
        if nn < 1e-9 {
            return (lam, tol, out);
        }
        let n = [n[0] / nn, n[1] / nn, n[2] / nn];
        for s in &self.showers {
            let d = wrap180(lam - s.max);
            if d < wrap180(s.begin - s.max) || d > wrap180(s.end - s.max) {
                continue; // not active
            }
            let ra = (s.ra + s.dra * d).rem_euclid(360.0);
            let dec = (s.dec + s.ddec * d).clamp(-90.0, 90.0);
            let r = vec3(ra, dec);
            let offset = dot(&r, &n).abs().min(1.0).asin().to_degrees();
            if offset > LIST_MAX_OFFSET_DEG {
                continue;
            }
            let d0 = dot(&r, v0).clamp(-1.0, 1.0).acos();
            let d1 = dot(&r, v1).clamp(-1.0, 1.0).acos();
            let behind = d1 > d0;
            out.push(Match {
                code: s.code.clone(),
                name: s.name.clone(),
                radiant_ra: ra,
                radiant_dec: dec,
                offset_deg: offset,
                behind,
                hit: behind && offset <= tol,
            });
        }
        out.sort_by(|a, b| a.offset_deg.total_cmp(&b.offset_deg));
        (lam, tol, out)
    }
}

/// Keeps the loaded list and re-reads it when the file changes.
#[derive(Default)]
pub struct CatalogCache {
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    catalog: Option<Catalog>,
}

impl CatalogCache {
    /// The current list (reloaded if the file changed), if any.
    pub fn get(&mut self) -> Option<&Catalog> {
        let path = [paths::SHOWERS_CSV, paths::SHOWERS_CSV_BUILTIN]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file());
        let stamp = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if path != self.path || stamp != self.stamp {
            self.catalog = path.as_deref().and_then(Catalog::from_file);
            self.path = path;
            self.stamp = stamp;
        }
        self.catalog.as_ref()
    }
}

impl Match {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "code": self.code,
            "name": self.name,
            "radiant_ra": self.radiant_ra,
            "radiant_dec": self.radiant_dec,
            "offset_deg": self.offset_deg,
            "behind": self.behind,
            "hit": self.hit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "# header\n\
#  No|Code|Name|Begin|Max|End|RA|dRA|Dec|dDec|Vg|dVg|Reference\n\
   17|NTA |Northern Taurids  |217  |224.5|241  | 52.3| 0.93| 21.9| 0.18|28.0|-0.07|GMN\n\
    2|STA |Southern Taurids  |190  |216  |230  | 46.6| 0.77| 12.9| 0.25|27.5|-0.06|GMN\n\
   21|AVB |alpha Virginids   |     | 32  |     |203.5| 0.91|  2.9|-0.36|18.8| 0.00|Jenniskens\n\
bad line without enough fields|x\n";

    #[test]
    fn parses_rms_format() {
        let s = Catalog::parse(CSV);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].code, "NTA");
        assert!((s[0].dra - 0.93).abs() < 1e-9);
        // missing begin/end fall back to max ± 5°
        assert!((s[2].begin - 27.0).abs() < 1e-9 && (s[2].end - 37.0).abs() < 1e-9);
    }

    #[test]
    fn solar_longitude_matches_known_dates() {
        // 2026-03-20 ≈ equinox (λ≈0°); 2026-11-12 12:00 UTC ≈ 230.1°
        let eq = 1_773_964_800.0; // 2026-03-20 00:00 UTC
        assert!(wrap180(solar_longitude(eq)).abs() < 1.0);
        let nov = 1_794_484_800.0; // 2026-11-12 12:00 UTC
        assert!((solar_longitude(nov) - 230.1).abs() < 0.3, "{}", solar_longitude(nov));
    }

    /// A meteor that lies on a great circle through the NTA radiant, moving away.
    #[test]
    fn hit_behind_radiant_and_miss_when_reversed() {
        let cat = Catalog { showers: Catalog::parse(CSV), info: json!({}) };
        let nov = 1_794_484_800.0;
        let lam = solar_longitude(nov);
        let nta = &cat.showers[0];
        let d = wrap180(lam - nta.max);
        let radiant = vec3(nta.ra + nta.dra * d, nta.dec + nta.ddec * d);
        // walk 40° and 48° away from the radiant along some great circle
        let t = {
            let up = [0.0, 0.0, 1.0];
            let c = [
                radiant[1] * up[2] - radiant[2] * up[1],
                radiant[2] * up[0] - radiant[0] * up[2],
                radiant[0] * up[1] - radiant[1] * up[0],
            ];
            let n = dot(&c, &c).sqrt();
            [c[0] / n, c[1] / n, c[2] / n]
        };
        let at = |deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            [
                c * radiant[0] + s * t[0],
                c * radiant[1] + s * t[1],
                c * radiant[2] + s * t[2],
            ]
        };
        let (v0, v1) = (at(40.0), at(48.0));
        let (_, tol, m) = cat.associate(&v0, &v1, 55.0, nov);
        assert!(tol > 5.0 && tol < 7.0, "tol {tol}");
        let nta_m = m.iter().find(|x| x.code == "NTA").unwrap();
        assert!(nta_m.hit && nta_m.offset_deg < 0.01, "{:?}", nta_m.offset_deg);
        // AVB is not active in November
        assert!(m.iter().all(|x| x.code != "AVB"));
        // reversed direction: radiant is in front, so not a hit
        let (_, _, m) = cat.associate(&v1, &v0, 55.0, nov);
        assert!(!m.iter().find(|x| x.code == "NTA").unwrap().hit);
    }

    #[test]
    fn short_tracks_get_wider_tolerance() {
        let cat = Catalog { showers: Catalog::parse(CSV), info: json!({}) };
        let (a, b) = ([1.0, 0.0, 0.0], [0.99, 0.1, 0.0]);
        let (_, t_long, _) = cat.associate(&a, &b, 80.0, 1.8e9);
        let (_, t_short, _) = cat.associate(&a, &b, 12.0, 1.8e9);
        assert!(t_short > t_long && t_short <= TOL_CAP_DEG);
    }
}
