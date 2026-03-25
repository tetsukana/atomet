//! Userspace interface to /dev/atomet kernel module.
//!
//! Mirrors atomet.h ioctl definitions. All operations are
//! simple file open + ioctl, no dependencies beyond libc.

use isvp_sys::*;
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
}

/// Apply AE manual attributes (integration time, analog gain, digital gain).
/// A value of 0 means auto for that parameter.
pub unsafe fn apply_ae_manual(exposure_us: u32, analog_gain: u32, digital_gain: u32) {
    let mut ae: IMPISPAEAttr = unsafe { std::mem::zeroed() };

    ae.AeFreezenEn = IMPISPTuningOpsMode_IMPISP_TUNING_OPS_MODE_ENABLE;
    // Integration time
    if exposure_us > 0 {
        ae.AeItManualEn = IMPISPTuningOpsMode_IMPISP_TUNING_OPS_MODE_ENABLE;
        ae.AeIt = exposure_us;
        ae.AeFreezenEn = IMPISPTuningOpsMode_IMPISP_TUNING_OPS_MODE_ENABLE;
    }

    // Analog gain
    if analog_gain > 0 {
        ae.AeAGainManualEn = IMPISPTuningOpsMode_IMPISP_TUNING_OPS_MODE_ENABLE;
        ae.AeAGain = analog_gain;
    }

    // Digital gain
    if digital_gain > 0 {
        ae.AeDGainManualEn = IMPISPTuningOpsMode_IMPISP_TUNING_OPS_MODE_ENABLE;
        ae.AeDGain = digital_gain;
    }

    let ret = unsafe { IMP_ISP_Tuning_SetAeAttr(&mut ae) };
    if ret != 0 {
        log::warn!("IMP_ISP_Tuning_SetAeAttr failed: {}", ret);
    }
}
