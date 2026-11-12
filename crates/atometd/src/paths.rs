//! Centralized filesystem paths under `/media/mmc/`.
//!
//! All on-disk locations are derived from this module so the storage layout
//! has a single source of truth.
//!
//! Layout:
//! ```text
//! /media/mmc/
//! ├── config/
//! │   ├── atomet.toml
//! │   ├── mask.bin
//! │   ├── solver-db.bin
//! │   ├── solve-lock.json
//! │   └── sensor-cal.bin
//! ├── logs/
//! │   └── atomet.log[.1]
//! ├── web/                    (optional asset override)
//! ├── records/{Y/m/d/H}/      (regular HEVC)
//! ├── timelapse/{Y/m/d}/      (timelapse HEVC)
//! ├── detections/{Y/m/d/HMS}/ (detection HEVC clips)
//! ├── stills/{Y/m/d}/         (single-frame FITS captures)
//! └── ff/{Y/m/d/H}/           (FF FITS blocks, 256-frame stacks)
//! ```

use chrono::{DateTime, Local};
use std::path::PathBuf;

pub const ROOT: &str = "/media/mmc";

// --- Config / state ---------------------------------------------------------
pub const CONFIG_TOML: &str = "/media/mmc/config/atomet.toml";
pub const MASK_BIN: &str = "/media/mmc/config/mask.bin";
/// SD-card override for the plate solver database (takes priority).
pub const SOLVER_DB: &str = "/media/mmc/config/solver-db.bin";
/// Database baked into the rootfs at build time (see post_build.sh).
pub const SOLVER_DB_BUILTIN: &str = "/usr/share/atomet/solver-db.bin";
/// Last validated plate solution (attitude + lens), see `wcs::Reference`.
pub const SOLVE_LOCK: &str = "/media/mmc/config/solve-lock.json";
pub const SENSOR_CAL: &str = "/media/mmc/config/sensor-cal.bin";
pub const WEB_OVERRIDE: &str = "/media/mmc/web";

// --- Logs -------------------------------------------------------------------
pub const LOG_FILE: &str = "/media/mmc/logs/atomet.log";
pub const LOG_FILE_OLD: &str = "/media/mmc/logs/atomet.log.1";
pub const LOG_DIR: &str = "/media/mmc/logs";

// --- Time-series data dirs (date-hierarchical) ------------------------------
pub const RECORDS_DIR: &str = "/media/mmc/records";
pub const TIMELAPSE_DIR: &str = "/media/mmc/timelapse";
pub const DETECTIONS_DIR: &str = "/media/mmc/detections";
pub const STILLS_DIR: &str = "/media/mmc/stills";
pub const FF_DIR: &str = "/media/mmc/ff";
pub const FF_DETECTIONS_DIR: &str = "/media/mmc/ff/detections";

// --- Date-hierarchy helpers -------------------------------------------------

pub fn record_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", RECORDS_DIR, t.format("%Y/%m/%d/%H")))
}

pub fn timelapse_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", TIMELAPSE_DIR, t.format("%Y/%m/%d")))
}

pub fn detection_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", DETECTIONS_DIR, t.format("%Y/%m/%d/%H%M%S")))
}

pub fn stills_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", STILLS_DIR, t.format("%Y/%m/%d")))
}

pub fn ff_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", FF_DIR, t.format("%Y/%m/%d/%H")))
}

pub fn ff_detections_dir(t: DateTime<Local>) -> PathBuf {
    PathBuf::from(format!("{}/{}", FF_DETECTIONS_DIR, t.format("%Y/%m/%d/%H")))
}

/// FF filename in RMS-style: `FF_YYYYMMDD_HHMMSS_mmm.fits` (mmm = milliseconds).
pub fn ff_filename(t: DateTime<Local>) -> String {
    format!("FF_{}.fits", t.format("%Y%m%d_%H%M%S_%3f"))
}

/// `still_YYYYMMDD_HHMMSS.fits`
pub fn still_filename(t: DateTime<Local>) -> String {
    format!("still_{}.fits", t.format("%Y%m%d_%H%M%S"))
}

/// Disk-cleanup priority order (least-valuable first).
pub const CLEANUP_DIRS: &[&str] = &[
    RECORDS_DIR,
    TIMELAPSE_DIR,
    STILLS_DIR,
    FF_DIR,
    DETECTIONS_DIR,
];
