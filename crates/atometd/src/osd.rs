use isvp_sys::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::SharedAppState;
use crate::font::{BITMAP_ARRAY, CHAR_HEIGHT, CHAR_WIDTH};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const TEXT_BUF_SIZE: usize = 43;

static mut FONT_HANDLE: IMPRgnHandle = 0;

static mut GRP_NUM: i32 = 0;

const DEFAULT_TIMESTAMP_POS: TimestampPos = TimestampPos::BottomLeft;

#[derive(Copy, Clone)]
pub enum TimestampPos {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl TimestampPos {
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => TimestampPos::TopLeft,
            1 => TimestampPos::TopRight,
            2 => TimestampPos::BottomLeft,
            3 => TimestampPos::BottomRight,
            _ => {
                log::warn!(
                    "Invalid timestamp_position {}, defaulting to BottomLeft",
                    value
                );
                TimestampPos::BottomLeft
            }
        }
    }
}

fn set_osd_attr(timestamp_pos: TimestampPos) -> bool {
    let mut rect = unsafe { std::mem::zeroed::<IMPRect>() };

    match timestamp_pos {
        TimestampPos::TopLeft => {
            rect.p0.x = (CHAR_WIDTH + 2) as i32;
            rect.p0.y = 5;
            rect.p1.x = ((CHAR_WIDTH + 2) + TEXT_BUF_SIZE * (CHAR_WIDTH + 2) - 1) as i32;
            rect.p1.y = 20;
        }
        TimestampPos::TopRight => {
            rect.p0.x = 1920 - ((TEXT_BUF_SIZE) * (CHAR_WIDTH + 2)) as i32;
            rect.p0.y = 5;
            rect.p1.x = 1920 - 1;
            rect.p1.y = 20;
        }
        TimestampPos::BottomLeft => {
            rect.p0.x = (CHAR_WIDTH + 2) as i32;
            rect.p0.y = 1080 - 21;
            rect.p1.x = ((CHAR_WIDTH + 2) + TEXT_BUF_SIZE * (CHAR_WIDTH + 2) - 1) as i32;
            rect.p1.y = 1080 - 6;
        }
        TimestampPos::BottomRight => {
            rect.p0.x = 1920 - ((TEXT_BUF_SIZE) * (CHAR_WIDTH + 2)) as i32;
            rect.p0.y = 1080 - 21;
            rect.p1.x = 1920 - 1;
            rect.p1.y = 1080 - 6;
        }
    };

    let mut font_attr = IMPOSDRgnAttr {
        type_: IMPOsdRgnType_OSD_REG_BITMAP,
        rect,
        fmt: IMPPixelFormat_PIX_FMT_MONOWHITE,
        data: IMPOSDRgnAttrData {
            bitmapData: std::ptr::null_mut(),
        },
    };

    if unsafe { IMP_OSD_SetRgnAttr(FONT_HANDLE, &mut font_attr) } < 0 {
        log::error!("IMP_OSD_SetRgnAttr failed");
        return false;
    }

    true
}

/// Initialize the OSD subsystem: create OSD group, register font region, set attributes, bind to FS.
unsafe fn osd_init() -> bool {
    let grp_num = 0;
    let font_handle = unsafe { IMP_OSD_CreateRgn(std::ptr::null_mut()) };
    let mut gr_font_attr = unsafe { std::mem::zeroed::<IMPOSDGrpRgnAttr>() };

    let mut osd_cell = IMPCell {
        deviceID: IMPDeviceID_DEV_ID_OSD,
        groupID: grp_num,
        outputID: 0,
    };

    let mut fs_cell = IMPCell {
        deviceID: IMPDeviceID_DEV_ID_FS,
        groupID: grp_num,
        outputID: 0,
    };

    if unsafe { IMP_OSD_CreateGroup(grp_num) } < 0 {
        log::error!("IMP_OSD_CreateGroup failed");
        return false;
    }

    if unsafe { IMP_OSD_RegisterRgn(font_handle, grp_num, std::ptr::null_mut()) } < 0 {
        log::error!("IMP_OSD_RegisterRgn failed");
        return false;
    }

    if !set_osd_attr(DEFAULT_TIMESTAMP_POS) {
        log::error!("Failed to set initial OSD attributes");
        return false;
    }

    if unsafe { IMP_OSD_GetGrpRgnAttr(font_handle, grp_num, &mut gr_font_attr) } < 0 {
        log::error!("IMP_OSD_GetGrpRgnAttr failed");
        return false;
    }

    gr_font_attr.show = 0;
    gr_font_attr.gAlphaEn = 1;
    gr_font_attr.fgAlhpa = 0xff;
    gr_font_attr.layer = 3;

    gr_font_attr.scalex = 3.;
    gr_font_attr.scaley = 3.;
    gr_font_attr.bgAlhpa = 0;
    gr_font_attr.offPos = IMPPoint { x: 0, y: 0 };

    if unsafe { IMP_OSD_SetGrpRgnAttr(font_handle, grp_num, &mut gr_font_attr) } < 0 {
        log::error!("IMP_OSD_SetGrpRgnAttr failed");
        return false;
    }

    if unsafe { IMP_OSD_Start(grp_num) } < 0 {
        log::error!("IMP_OSD_Start failed");
        return false;
    }

    if unsafe { IMP_System_Bind(&mut fs_cell, &mut osd_cell) } < 0 {
        log::error!("IMP_System_Bind failed");
        return false;
    }

    // Bind the OSD region to both encoder channels (streaming and recording) so it shows up on both outputs
    let mut encoder_cell_stream = IMPCell {
        deviceID: IMPDeviceID_DEV_ID_ENC,
        groupID: 0,
        outputID: 0,
    };

    if unsafe { IMP_System_Bind(&mut osd_cell, &mut encoder_cell_stream) } < 0 {
        log::error!("IMP_System_Bind failed");
        return false;
    }

    unsafe {
        FONT_HANDLE = font_handle;
        GRP_NUM = grp_num;
    }

    true
}

/// Exit the OSD subsystem: unbind from FS, unregister region, destroy group.
unsafe fn osd_exit() {
    let mut osd_cell = IMPCell {
        deviceID: IMPDeviceID_DEV_ID_OSD,
        groupID: unsafe { GRP_NUM },
        outputID: 0,
    };

    let mut fs_cell = IMPCell {
        deviceID: IMPDeviceID_DEV_ID_FS,
        groupID: 0,
        outputID: 0,
    };

    if unsafe { IMP_System_UnBind(&mut fs_cell, &mut osd_cell) } < 0 {
        log::error!("IMP_System_UnBind failed");
    }

    if unsafe { IMP_OSD_ShowRgn(FONT_HANDLE, GRP_NUM, 0) } < 0 {
        log::error!("IMP_OSD_ShowRgn failed");
    }

    if unsafe { IMP_OSD_UnRegisterRgn(FONT_HANDLE, GRP_NUM) } < 0 {
        log::error!("IMP_OSD_UnRegisterRgn failed");
    }

    unsafe { IMP_OSD_DestroyRgn(FONT_HANDLE) };

    if unsafe { IMP_OSD_DestroyGroup(GRP_NUM) } < 0 {
        log::error!("IMP_OSD_DestroyGroup failed");
    }
}

unsafe fn imp_osd_toggle_show(show: bool) -> bool {
    if unsafe { IMP_OSD_ShowRgn(FONT_HANDLE, GRP_NUM, if show { 1 } else { 0 }) } < 0 {
        log::error!("IMP_OSD_ShowRgn failed");
        return false;
    }
    true
}

/// OSD task: polls OSD for updates and applies them (e.g. text, timestamp).
///
///
pub fn osd_poll_loop(app_state: SharedAppState, shutdown: Arc<AtomicBool>) {
    let mut timestamp_data = vec![0u8; TEXT_BUF_SIZE * (CHAR_WIDTH + 2) * CHAR_HEIGHT];

    if unsafe { !osd_init() } {
        log::error!("OSD init failed");
        return;
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            log::info!("osd_poll_loop shutting down");
            break;
        }

        let state = app_state.load();

        if !set_osd_attr(TimestampPos::from_u32(state.timestamp_position)) {
            log::error!("Failed to update OSD attributes");
            continue;
        }

        unsafe {
            imp_osd_toggle_show(state.show_timestamp);
        }

        if state.show_timestamp {
            timestamp_data.fill(0);
            let now = chrono::Local::now();
            let fractional_second = (now.timestamp_subsec_millis() as f64) / 100.0;

            let text = format!(
                "{}.{} {}  ATOMET v{}",
                now.format("%Y-%m-%d %H:%M:%S"),
                fractional_second as u8,
                now.format("%:z"),
                VERSION
            );

            osd_update_text(&text, &mut timestamp_data);

            let mut data = IMPOSDRgnAttrData {
                bitmapData: timestamp_data.as_mut_ptr() as *mut ::std::os::raw::c_void,
            };

            unsafe {
                IMP_OSD_UpdateRgnAttrData(FONT_HANDLE, &mut data);
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    unsafe { osd_exit() };
}

fn osd_update_text(text: &str, timestamp_data: &mut [u8]) {
    for (i, c) in text.chars().enumerate() {
        let char_index = match c {
            '.' => 0,
            '-' => 49,
            '+' => 50,
            ':' => 12,
            '0'..='9' => 2 + (c as usize - '0' as usize),
            'A'..='Z' => 19 + (c as usize - 'A' as usize),
            'v' => 51,
            ' ' => continue,
            _ => {
                log::warn!("{} is not in the bitmap", c);
                continue;
            }
        };

        let base_offset = i * (CHAR_WIDTH + 1);
        for j in 0..CHAR_HEIGHT {
            let char_line = &BITMAP_ARRAY[(char_index * CHAR_HEIGHT + j) * CHAR_WIDTH
                ..(char_index * CHAR_HEIGHT + j + 1) * CHAR_WIDTH];
            timestamp_data[j * (CHAR_WIDTH + 2) * TEXT_BUF_SIZE + base_offset + 1
                ..j * (CHAR_WIDTH + 2) * TEXT_BUF_SIZE + base_offset + (CHAR_WIDTH + 1)]
                .copy_from_slice(char_line);
        }
    }
}
