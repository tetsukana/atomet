#[link(name = "imgproc", kind = "static")]
unsafe extern "C" {
    pub unsafe fn add_saturate(src1: *const u8, src2: *const u8, dst: *mut u8, n: usize);
    pub unsafe fn sub_saturate(src1: *const u8, src2: *const u8, dst: *mut u8, n: usize);
    pub unsafe fn abs_diff(src1: *const u8, src2: *const u8, dst: *mut u8, n: usize);
    pub unsafe fn brightest(src1: *const u8, src2: *const u8, dst: *mut u8, n: usize);
    pub unsafe fn mean_stddev(src: *const u8, n: usize, mean: *mut f64, stddev: *mut f64);
    pub unsafe fn bin_8x8(src: *const u8, width: usize, height: usize, dst: *const u8);
    pub unsafe fn bin_16x16(src: *const u8, width: usize, height: usize, dst: *const u8);
    pub unsafe fn stack_init(src: *const u8, acc: *mut u16, n: usize);
    pub unsafe fn stack_add(src: *const u8, acc: *mut u16, n: usize);

    // Star extraction (morphological peak detection + voting)
    pub unsafe fn stars_process_row(
        prev: *const u8,
        curr: *const u8,
        next: *const u8,
        vote_row: *mut u8,
        threshold: u8,
    );
    pub unsafe fn stars_calibrate_row(
        prev: *const u8,
        curr: *const u8,
        next: *const u8,
        hist: *mut u32,
    );
}
