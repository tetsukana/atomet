use chrono::{Local, Timelike};
use isvp_sys::*;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc};

use crate::config::AppState;
use crate::watchdog::WatchdogHandle;

const SENSOR_NAME: &[u8] = b"gc2053";
const SENSOR_WIDTH: i32 = 1920;
const SENSOR_HEIGHT: i32 = 1080;
const BITRATE_720P_KBS: u32 = 1000;

/// Initialize the ISP pipeline: ISP open, sensor, system init, tuning.
///
/// Must be called from a blocking context (spawn_blocking).
/// Follows the same init order as atomskygaze:
///   OSD pool → ISP_Open → AddSensor → EnableSensor → System_Init → EnableTuning → tuning params
pub unsafe fn isp_init(app_state: &AppState) -> bool {
    unsafe {
        IMP_OSD_SetPoolSize(512 * 1024);
    }

    // Prepare sensor info
    let mut sensor_info: IMPSensorInfo = unsafe { std::mem::zeroed() };
    unsafe {
        sensor_info.name[..SENSOR_NAME.len()]
            .copy_from_slice(std::mem::transmute::<&[u8], &[i8]>(SENSOR_NAME));
    }
    sensor_info.cbus_type = IMPSensorControlBusType_TX_SENSOR_CONTROL_INTERFACE_I2C;
    unsafe {
        let i2c = &mut sensor_info.__bindgen_anon_1.i2c;
        i2c.type_[..SENSOR_NAME.len()]
            .copy_from_slice(std::mem::transmute::<&[u8], &[i8]>(SENSOR_NAME));
        i2c.addr = 0x37;
        i2c.i2c_adapter_id = 0;
    }
    sensor_info.rst_gpio = 0;
    sensor_info.pwdn_gpio = 0;
    sensor_info.power_gpio = 0;

    log::info!("IMP_ISP_Open");
    if unsafe { IMP_ISP_Open() } < 0 {
        log::error!("IMP_ISP_Open failed");
        return false;
    }

    // Use ISP bin from SD card if present (allows tuning without rebuild)
    if std::path::Path::new("/media/mmc/gc2053-t31.bin").exists() {
        let path = b"/media/mmc/gc2053-t31.bin\0";
        unsafe { IMP_ISP_SetDefaultBinPath(path.as_ptr() as *mut _) };
        log::info!("ISP bin path: /media/mmc/gc2053-t31.bin (SD card override)");
    } else {
        log::info!("ISP bin path: /etc/sensor/gc2053-t31.bin (default)");
    }

    log::info!("IMP_ISP_AddSensor");
    if unsafe { IMP_ISP_AddSensor(&mut sensor_info) } < 0 {
        log::error!("IMP_ISP_AddSensor failed");
        return false;
    }

    log::info!("IMP_ISP_EnableSensor");
    if unsafe { IMP_ISP_EnableSensor() } < 0 {
        log::error!("IMP_ISP_EnableSensor failed");
        return false;
    }

    log::info!("IMP_System_Init");
    if unsafe { IMP_System_Init() } < 0 {
        log::error!("IMP_System_Init failed");
        return false;
    }

    log::info!("IMP_ISP_EnableTuning");
    if unsafe { IMP_ISP_EnableTuning() } < 0 {
        log::error!("IMP_ISP_EnableTuning failed");
        return false;
    }

    // Apply tuning parameters from config
    unsafe {
        IMP_ISP_Tuning_SetAntiFlickerAttr(IMPISPAntiflickerAttr_IMPISP_ANTIFLICKER_DISABLE);

        let mode = if app_state.night_mode {
            IMPISPRunningMode_IMPISP_RUNNING_MODE_NIGHT
        } else {
            IMPISPRunningMode_IMPISP_RUNNING_MODE_DAY
        };
        IMP_ISP_Tuning_SetISPRunningMode(mode);

        IMP_ISP_Tuning_SetSensorFPS(app_state.fps, 1);
    }

    true
}

/// Initialize the framesource channel 0 (1920x1080 NV12).
/// Matches atomskygaze's CHANNEL_ATTRIBUTES[0] exactly.
pub unsafe fn framesource_init() -> bool {
    // Create full HD NV12 channel on framesource 0
    let mut chn_attr_hd = unsafe { std::mem::zeroed::<IMPFSChnAttr>() };
    chn_attr_hd.picWidth = SENSOR_WIDTH;
    chn_attr_hd.picHeight = SENSOR_HEIGHT;
    chn_attr_hd.pixFmt = IMPPixelFormat_PIX_FMT_NV12;
    chn_attr_hd.crop.enable = 1;
    chn_attr_hd.crop.width = SENSOR_WIDTH;
    chn_attr_hd.crop.height = SENSOR_HEIGHT;
    chn_attr_hd.outFrmRateNum = 25;
    chn_attr_hd.outFrmRateDen = 1;
    chn_attr_hd.nrVBs = 2;
    chn_attr_hd.type_ = IMPFSChnType_FS_PHY_CHANNEL;

    // Create small NV12 channel on framesource 1 (used for motion detection)
    let mut chn_attr_sd = unsafe { std::mem::zeroed::<IMPFSChnAttr>() };
    chn_attr_sd.picWidth = 640;
    chn_attr_sd.picHeight = 360;
    chn_attr_sd.pixFmt = IMPPixelFormat_PIX_FMT_NV12;
    chn_attr_sd.crop.width = SENSOR_WIDTH;
    chn_attr_sd.crop.height = SENSOR_HEIGHT;
    chn_attr_sd.scaler.enable = 1;
    chn_attr_sd.scaler.outwidth = 640;
    chn_attr_sd.scaler.outheight = 360;
    chn_attr_sd.outFrmRateNum = 25;
    chn_attr_sd.outFrmRateDen = 1;
    chn_attr_sd.nrVBs = 2;
    chn_attr_sd.type_ = IMPFSChnType_FS_PHY_CHANNEL;

    // Create and set attributes for both channels
    // Create channel 0
    if unsafe { IMP_FrameSource_CreateChn(0, &mut chn_attr_hd) } < 0 {
        log::error!("IMP_FrameSource_CreateChn(0) failed");
        return false;
    }
    if unsafe { IMP_FrameSource_SetChnAttr(0, &chn_attr_hd) } < 0 {
        log::error!("IMP_FrameSource_SetChnAttr(0) failed");
        return false;
    }

    // Create channel 1
    if unsafe { IMP_FrameSource_CreateChn(1, &mut chn_attr_sd) } < 0 {
        log::error!("IMP_FrameSource_CreateChn(1) failed");
        return false;
    }
    if unsafe { IMP_FrameSource_SetChnAttr(1, &chn_attr_sd) } < 0 {
        log::error!("IMP_FrameSource_SetChnAttr(1) failed");
        return false;
    }

    true
}

/// Initialize HEVC encoder on channel 0.
/// Uses HEVC_MAIN + VBR, matching atomskygaze's imp_encoder_init().
pub unsafe fn encoder_init() -> bool {
    let mut encoder_attr = unsafe { std::mem::zeroed::<IMPEncoderChnAttr>() };

    if unsafe { IMP_Encoder_CreateGroup(0) } < 0 {
        log::error!("IMP_Encoder_CreateGroup(0) failed");
        return false;
    }

    let bitrate: f32 = ((SENSOR_WIDTH * SENSOR_HEIGHT) as f32 / (1280.0 * 720.0)).log10() + 1.0;
    let target_bitrate = (BITRATE_720P_KBS as f32 * bitrate) as u32;

    let ratio = 1.0 / (f32::log10((1920. * 1080.) / (640. * 360.)) + 1.0);
    let bitrate = (BITRATE_720P_KBS as f32 * ratio) as u32;

    println!("bitrate {}, target_bitrate {}", bitrate, target_bitrate);

    // Create a streaming channel on encoder group 0, bound to framesource channel 0
    if unsafe {
        IMP_Encoder_SetDefaultParam(
            &mut encoder_attr,
            IMPEncoderProfile_IMP_ENC_PROFILE_HEVC_MAIN,
            IMPEncoderRcMode_IMP_ENC_RC_MODE_FIXQP,
            SENSOR_WIDTH as u16,
            SENSOR_HEIGHT as u16,
            25,
            1,
            50,
            2,
            38,
            bitrate,
        )
    } < 0
    {
        log::error!("IMP_Encoder_SetDefaultParam failed");
        return false;
    }

    if unsafe { IMP_Encoder_CreateChn(0, &encoder_attr) } < 0 {
        log::error!("IMP_Encoder_CreateChn(0) failed");
        return false;
    }

    if unsafe { IMP_Encoder_RegisterChn(0, 0) } < 0 {
        log::error!("IMP_Encoder_RegisterChn(0, 0) failed");
        return false;
    }

    // Timelapse channel with lower bitrate
    let mut encoder_attr = unsafe { std::mem::zeroed::<IMPEncoderChnAttr>() };

    if unsafe { IMP_Encoder_CreateGroup(1) } < 0 {
        log::error!("IMP_Encoder_CreateGroup(1) failed");
        return false;
    }

    println!("bitrate {}, target_bitrate {}", bitrate, target_bitrate);

    // Create a streaming channel on encoder group 1, bound to framesource channel 0
    if unsafe {
        IMP_Encoder_SetDefaultParam(
            &mut encoder_attr,
            IMPEncoderProfile_IMP_ENC_PROFILE_HEVC_MAIN,
            IMPEncoderRcMode_IMP_ENC_RC_MODE_FIXQP,
            SENSOR_WIDTH as u16,
            SENSOR_HEIGHT as u16,
            1,
            1,
            5,
            2,
            38,
            bitrate,
        )
    } < 0
    {
        log::error!("IMP_Encoder_SetDefaultParam failed");
        return false;
    }

    if unsafe { IMP_Encoder_CreateChn(1, &encoder_attr) } < 0 {
        log::error!("IMP_Encoder_CreateChn(1) failed");
        return false;
    }

    if unsafe { IMP_Encoder_RegisterChn(0, 1) } < 0 {
        log::error!("IMP_Encoder_RegisterChn(0, 1) failed");
        return false;
    }

    true
}

/// Start the framesource (begins capturing frames).
pub unsafe fn framesource_start() -> bool {
    if unsafe { IMP_FrameSource_EnableChn(0) } < 0 {
        log::error!("IMP_FrameSource_EnableChn(0) failed");
        return false;
    }

    if unsafe { IMP_FrameSource_EnableChn(1) } < 0 {
        log::error!("IMP_FrameSource_EnableChn(1) failed");
        return false;
    }

    true
}

fn pad_to_2880(mut v: Vec<u8>) -> Vec<u8> {
    let pad = (2880 - (v.len() % 2880)) % 2880;
    v.extend(vec![b' '; pad]);
    v
}

fn make_card(s: &str) -> Vec<u8> {
    let mut buf = vec![b' '; 80];
    let bytes = s.as_bytes();
    buf[..bytes.len()].copy_from_slice(bytes);
    buf
}

fn save_to_fits(data: Vec<u8>) {
    let mut header = Vec::new();

    header.extend(make_card("SIMPLE  =                    T"));
    header.extend(make_card("BITPIX  =                    8"));
    header.extend(make_card("NAXIS   =                    2"));
    header.extend(make_card("NAXIS1  =                 1920"));
    header.extend(make_card("NAXIS2  =                 1080"));
    header.extend(make_card("END"));

    header = pad_to_2880(header);

    let data = pad_to_2880(data);

    let now = Local::now();
    let path = format!("/media/mmc/fits/{}.fits", now.format("%Y%m%d_%H%M%S"));

    let mut file = File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&data).unwrap();
}

/// Capture a single 1920×1080 Y-plane from framesource channel 0.
/// Must be called from a blocking context.
pub unsafe fn capture_luma_1080p() {
    let frame_size = (SENSOR_WIDTH * SENSOR_HEIGHT) as usize;

    // Enable raw frame access on ch0 (alongside encoder)
    if unsafe { IMP_FrameSource_SetFrameDepth(0, 1) } < 0 {
        log::error!("capture_luma_1080p: SetFrameDepth(0,1) failed");
    }

    let mut frame_ptr: *mut IMPFrameInfo = std::ptr::null_mut();
    if unsafe { IMP_FrameSource_GetFrame(0, &mut frame_ptr) } == 0 {
        let src = unsafe { (*frame_ptr).virAddr as *const u8 };
        let data = unsafe { std::slice::from_raw_parts(src, frame_size) }.to_vec();
        unsafe { IMP_FrameSource_ReleaseFrame(0, frame_ptr) };
        save_to_fits(data);
    } else {
        log::error!("capture_luma_1080p: GetFrame(0) failed");
    }

    // Disable raw frame access
    unsafe { IMP_FrameSource_SetFrameDepth(0, 0) };
}

/// Log all ISP tuning parameters for debugging.
pub unsafe fn log_isp_values() {
    unsafe {
        let mut vu8: libc::c_uchar = 0;

        IMP_ISP_Tuning_GetBrightness(&mut vu8);
        log::debug!("brightness {}", vu8);
        IMP_ISP_Tuning_GetContrast(&mut vu8);
        log::debug!("contrast {}", vu8);
        IMP_ISP_Tuning_GetSharpness(&mut vu8);
        log::debug!("sharpness {}", vu8);
        IMP_ISP_Tuning_GetSaturation(&mut vu8);
        log::debug!("saturation {}", vu8);

        let mut vu32: u32 = 0;
        IMP_ISP_Tuning_GetTotalGain(&mut vu32);
        log::debug!("total gain {}", vu32);
        IMP_ISP_Tuning_GetISPHflip(&mut vu32);
        log::debug!("hflip {}", vu32);
        IMP_ISP_Tuning_GetISPVflip(&mut vu32);
        log::debug!("vflip {}", vu32);
    }
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub packs: Vec<VideoPack>,
}

#[derive(Debug, Clone)]
pub struct VideoPack {
    pub nal_type: u32,
    pub data: Vec<u8>,
}

/// Poll H.265 encoded frames from the ISP encoder and broadcast via watch channel.
pub unsafe fn video_poll_worker(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    tx: broadcast::Sender<Arc<VideoFrame>>,
) {
    let mut current_minute = 99;
    if unsafe { IMP_Encoder_StartRecvPic(0) } < 0 {
        log::error!("IMP_Encoder_StartRecvPic(0) failed");
        return;
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            log::info!("video_poll_worker shutting down");
            break;
        }

        wd.tick();

        let now = Local::now();
        if now.minute() != current_minute {
            // Request IDR frame on minute change to ensure clean keyframe for new record
            unsafe { IMP_Encoder_RequestIDR(0) };
            current_minute = now.minute();
        }

        if unsafe { IMP_Encoder_PollingStream(0, 1000) } < 0 {
            continue; // timeout, try again
        }

        let mut stream: IMPEncoderStream = unsafe { std::mem::zeroed() };
        if unsafe { IMP_Encoder_GetStream(0, &mut stream, true) } < 0 {
            log::error!("IMP_Encoder_GetStream failed");
            continue;
        }

        let packs = unsafe {
            std::slice::from_raw_parts(
                stream.pack as *const IMPEncoderPack,
                stream.packCount as usize,
            )
        };
        let mut video_packs = Vec::with_capacity(stream.packCount as usize);
        for pack in packs {
            if pack.length == 0 {
                continue;
            }

            let data = unsafe { get_pack_data(&stream, pack) };

            video_packs.push(VideoPack {
                nal_type: unsafe { pack.nalType.h265NalType },
                data,
            });
        }

        unsafe { IMP_Encoder_ReleaseStream(0, &mut stream) };

        let video_frame = Arc::new(VideoFrame { packs: video_packs });
        if tx.send(video_frame).is_err() {
            log::error!("Failed to send video frame: receiver dropped");
            break;
        };
    }

    unsafe { IMP_Encoder_StopRecvPic(0) };
}

pub unsafe fn timelapse_poll_worker(
    shutdown: Arc<AtomicBool>,
    wd: WatchdogHandle,
    tx: mpsc::Sender<Arc<VideoFrame>>,
) {
    let mut current_hour = 99;
    if unsafe { IMP_Encoder_StartRecvPic(1) } < 0 {
        log::error!("IMP_Encoder_StartRecvPic(1) failed");
        return;
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            log::info!("timelapse_poll_worker shutting down");
            break;
        }

        wd.tick();

        let now = Local::now();
        if now.hour() != current_hour {
            // Request IDR frame on hour change to ensure clean keyframe for new record
            unsafe { IMP_Encoder_RequestIDR(1) };
            current_hour = now.hour();
        }

        if unsafe { IMP_Encoder_PollingStream(1, 10000) } < 0 {
            continue; // timeout, try again
        }

        let mut stream: IMPEncoderStream = unsafe { std::mem::zeroed() };
        if unsafe { IMP_Encoder_GetStream(1, &mut stream, true) } < 0 {
            log::error!("IMP_Encoder_GetStream failed");
            continue;
        }

        let packs = unsafe {
            std::slice::from_raw_parts(
                stream.pack as *const IMPEncoderPack,
                stream.packCount as usize,
            )
        };
        let mut video_packs = Vec::with_capacity(stream.packCount as usize);
        for pack in packs {
            if pack.length == 0 {
                continue;
            }

            let data = unsafe { get_pack_data(&stream, pack) };

            video_packs.push(VideoPack {
                nal_type: unsafe { pack.nalType.h265NalType },
                data,
            });
        }

        unsafe { IMP_Encoder_ReleaseStream(1, &mut stream) };

        let video_frame = Arc::new(VideoFrame { packs: video_packs });
        if tx.blocking_send(video_frame).is_err() {
            log::error!("Failed to send video frame: receiver dropped");
            break;
        };
    }

    unsafe { IMP_Encoder_StopRecvPic(1) };
}

unsafe fn get_pack_data(stream: &IMPEncoderStream, pack: &IMPEncoderPack) -> Vec<u8> {
    let rem_size = stream.streamSize - pack.offset;
    if rem_size < pack.length {
        let mut v = Vec::with_capacity(pack.length as usize);
        v.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                (stream.virAddr + pack.offset) as *const u8,
                rem_size as usize,
            )
        });
        v.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                stream.virAddr as *const u8,
                (pack.length - rem_size) as usize,
            )
        });
        v
    } else {
        unsafe {
            std::slice::from_raw_parts(
                (stream.virAddr + pack.offset) as *const u8,
                pack.length as usize,
            )
            .to_vec()
        }
    }
}
