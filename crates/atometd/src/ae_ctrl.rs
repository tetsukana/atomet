//! Userspace interface to /dev/atomet kernel module.
//!
//! Mirrors atomet.h ioctl definitions. All operations are
//! simple file open + ioctl, no dependencies beyond libc.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

const DEVICE_PATH: &str = "/dev/atomet";

// ioctl magic from atomet.h
const IOC_MAGIC: u32 = b'M' as u32;

// MIPS ioctl encoding — differs from generic Linux!
// arch/mips/include/uapi/asm/ioctl.h:
//   _IOC_SIZEBITS=13, _IOC_DIRBITS=3, _IOC_DIRSHIFT=29
//   _IOC_NONE=1, _IOC_READ=2, _IOC_WRITE=4
const fn ioc_read(nr: u32, size: u32) -> u32 {
    (2 << 29) | (IOC_MAGIC << 8) | nr | (size << 16)
}
const fn ioc_write(nr: u32, size: u32) -> u32 {
    (4 << 29) | (IOC_MAGIC << 8) | nr | (size << 16)
}

const ATOMET_GET_AE_ATTR: u32 = ioc_read(0, std::mem::size_of::<AeParams>() as u32);
const ATOMET_SET_AE_ATTR: u32 = ioc_write(1, std::mem::size_of::<AeParams>() as u32);
const ATOMET_GET_TOP_BYPASS: u32 = ioc_read(2, std::mem::size_of::<TopBypass>() as u32);
const ATOMET_SET_TOP_BYPASS: u32 = ioc_write(3, std::mem::size_of::<TopBypass>() as u32);
const ATOMET_SET_DAY_NIGHT: u32 = ioc_write(4, std::mem::size_of::<i32>() as u32);
// _IO on MIPS: dir=1 (IOC_NONE), size=0
const fn ioc_none(nr: u32) -> u32 {
    (1 << 29) | (IOC_MAGIC << 8) | nr
}
const ATOMET_FREEZE_AE: u32 = ioc_none(5);
const ATOMET_UNFREEZE_AE: u32 = ioc_none(6);
const ATOMET_SET_MAX_IDG: u32 = ioc_write(7, std::mem::size_of::<u32>() as u32);

/// tisp_ae_ctrls — 152 bytes, matches atomet.h struct ae_params
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AeParams {
    pub ae_mode: u32,
    pub ag_value: u32,
    pub sdg_value: u32,
    pub it_value: u32,
    pub idg_value: u32,
    pub max_ag: u32,
    pub max_sdg: u32,
    pub max_it: u32,
    pub max_idg: u32,
    pub total_ev: u32,
    pub total_gain_log2: u32,
    pub again_log2: u32,
    pub _pad: u32,
    pub it_manual_en: u32,
    pub ag_manual_en: u32,
    pub dg_manual_en: u32,
    pub s_it_en: u32,
    pub s_ag_en: u32,
    pub s_ag_val: u32,
    pub s_it_val: u32,
    pub max_ag_short: u32,
    pub max_it_short: u32,
    pub max_sdg_short: u32,
    pub max_idg_short: u32,
    pub s_freeze: u32,
    pub s_sdg: u32,
    pub s_sdg_manual: u32,
    pub s_idg: u32,
    pub min_ag: u32,
    pub min_sdg: u32,
    pub min_it: u32,
    pub min_idg: u32,
    pub min_ag_short: u32,
    pub min_it_short: u32,
    pub min_sdg_short: u32,
    pub min_idg_short: u32,
    pub sdg_en: u32,
    pub s_sdg_en: u32,
}

/// TOP_BYPASS — 32 × u32
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct TopBypass {
    pub bits: [u32; 32],
}
// GC2053 MIPI 25fps: 1 line = 30μs
const ONE_LINE_US: u32 = 30;

/// Exposure: lines → microseconds
pub fn lines_to_us(lines: u32) -> u32 {
    lines * ONE_LINE_US
}

/// Exposure: microseconds → lines
pub fn us_to_lines(us: u32) -> u32 {
    (us + ONE_LINE_US / 2) / ONE_LINE_US
}

/// ISP gain (log2 << 16) → 0.1 dB
/// dB = gain * 20 * log10(2) / 65536 = gain * 6.0206 / 65536
/// 0.1 dB = gain * 60.206 / 65536
pub fn isp_gain_to_db10(gain: u32) -> u32 {
    ((gain as u64 * 60206 + 65536 * 500) / (65536 * 1000)) as u32
}

/// 0.1 dB → ISP gain (log2 << 16)
pub fn db10_to_isp_gain(db10: u32) -> u32 {
    ((db10 as u64 * 65536 * 1000 + 30103) / 60206) as u32
}

pub struct AtometDev {
    fd: File,
}

impl AtometDev {
    pub fn open() -> io::Result<Self> {
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open(DEVICE_PATH)?;
        Ok(Self { fd })
    }

    pub fn get_ae(&self) -> io::Result<AeParams> {
        let mut params = AeParams::default();
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_GET_AE_ATTR as _, &mut params) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(params)
    }

    pub fn set_ae(&self, params: &AeParams) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_SET_AE_ATTR as _, params) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn get_bypass(&self) -> io::Result<TopBypass> {
        let mut bp = TopBypass::default();
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_GET_TOP_BYPASS as _, &mut bp) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bp)
    }

    pub fn set_bypass(&self, bp: &TopBypass) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_SET_TOP_BYPASS as _, bp) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn set_day_night(&self, mode: i32) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_SET_DAY_NIGHT as _, &mode) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn freeze_ae(&self) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_FREEZE_AE as _) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn unfreeze_ae(&self) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_UNFREEZE_AE as _) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Cap ISP digital gain. 0 = no digital gain (1×).
    /// Value in ISP gain format (log2 << 16). Use db10_to_isp_gain() to convert.
    pub fn set_max_idg(&self, val: u32) -> io::Result<()> {
        let ret = unsafe { libc::ioctl(self.fd.as_raw_fd(), ATOMET_SET_MAX_IDG as _, &val) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

