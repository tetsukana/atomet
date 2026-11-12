#include "imgproc.h"

/* ----------------------------------------------------------------
   SIMD実装 (MXU2)
   ---------------------------------------------------------------- */

void add_saturate(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n) {
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        v16i8 a = _mx128_lu1q((void *)(src1 + i), 0);
        v16i8 b = _mx128_lu1q((void *)(src2 + i), 0);
        v16i8 r = (v16i8)_mx128_adduu_b((v16u8)a, (v16u8)b);
        _mx128_su1q(r, (void *)(dst + i), 0);
    }
}

void sub_saturate(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n) {
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        v16i8 a = _mx128_lu1q((void *)(src1 + i), 0);
        v16i8 b = _mx128_lu1q((void *)(src2 + i), 0);
        v16i8 r = (v16i8)_mx128_subuu_b((v16u8)a, (v16u8)b);
        _mx128_su1q(r, (void *)(dst + i), 0);
    }
}

void abs_diff(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n) {
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        v16i8 a = _mx128_lu1q((void *)(src1 + i), 0);
        v16i8 b = _mx128_lu1q((void *)(src2 + i), 0);
        v16i8 r = (v16i8)_mx128_subua_b((v16u8)a, (v16u8)b);
        _mx128_su1q(r, (void *)(dst + i), 0);
    }
}

void brightest(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n) {
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        v16i8 a = _mx128_lu1q((void *)(src1 + i), 0);
        v16i8 b = _mx128_lu1q((void *)(src2 + i), 0);
        v16i8 r = (v16i8)_mx128_maxu_b((v16u8)a, (v16u8)b);
        _mx128_su1q(r, (void *)(dst + i), 0);
    }
}

void update_peak_frame(const uint8_t *src, uint8_t *dst, const uint8_t *brightest,
                       uint8_t frm_num, size_t n) {
    v16i8 frm_vec = _mx128_mfcpu_b((int)frm_num);
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        v16u8 s = (v16u8)_mx128_lu1q((void *)(src + i), 0);
        v16u8 b = (v16u8)_mx128_lu1q((void *)(brightest + i), 0);
        v16i8 d = _mx128_lu1q((void *)(dst + i), 0);
        /* mask = 0xFF where src > brightest, else 0x00 */
        v16i8 mask = _mx128_cltu_b(b, s);
        /* select: dst = mask ? frm_vec : dst  (via d ^ ((d ^ frm) & mask)) */
        v16i8 r = _mx128_xorv(d, _mx128_andv(_mx128_xorv(d, frm_vec), mask));
        _mx128_su1q(r, (void *)(dst + i), 0);
    }
}

void mean_stddev(const uint8_t *src, size_t length, double *mean, double *stddev) {
    size_t i;
    v16u8 unit8  = (v16u8)_mx128_li_b(1);
    v8u16 unit16 = (v8u16)_mx128_li_h(1);
    v4u32 unit32 = (v4u32)_mx128_li_w(1);
    v2u64 sum    = (v2u64)_mx128_li_d(0);
    v2u64 ssd    = (v2u64)_mx128_li_d(0);

    for (i = 0; i < length; i += VECTOR_SIZE) {
        v16u8 vec    = (v16u8)_mx128_lu1q((void *)(src + i), 0);
        v8u16 s16    = _mx128_dotpu_h(vec, unit8);
        v4u32 s32    = _mx128_dotpu_w(s16, unit16);
        sum          = _mx128_daddu_d(sum, s32, unit32);
    }

    *mean = (double)(_mx128_mtcpuu_d((v2i64)sum, 0) + _mx128_mtcpuu_d((v2i64)sum, 1)) / (double)length;
    v16u8 mean_vec = (v16u8)_mx128_mfcpu_b((int)*mean);

    for (i = 0; i < length; i += VECTOR_SIZE) {
        v16u8 vec = (v16u8)_mx128_lu1q((void *)(src + i), 0);
        v16u8 diff = (v16u8)_mx128_subua_b(vec, mean_vec);
        v8u16 s16  = _mx128_dotpu_h(diff, diff);
        v4u32 s32  = _mx128_dotpu_w(s16, unit16);
        ssd        = _mx128_daddu_d(ssd, s32, unit32);
    }

    *stddev = sqrt((double)(_mx128_mtcpuu_d((v2i64)ssd, 0) + _mx128_mtcpuu_d((v2i64)ssd, 1)) / (double)length);
}

void bin_8x8(const uint8_t *src, size_t width, size_t height, uint8_t *dst) {
    size_t bx, by, row;
    size_t grid_w = width / 8;   /* 80 */
    size_t grid_h = height / 8;  /* 45 */

    v16u8 unit8 = (v16u8)_mx128_li_b(1);
    v8u16 unit16 = (v8u16)_mx128_li_h(1);
    v4u32 unit32 = (v4u32)_mx128_li_w(1);

    for (by = 0; by < grid_h; by++) {
        for (bx = 0; bx < grid_w; bx += 2) {
            v4u32 acc32 = (v4u32)_mx128_li_w(0);

            for (row = 0; row < 8; row++) {
                const uint8_t *p = src + (by * 8 + row) * width + bx * 8;
                v16u8 v    = (v16u8)_mx128_lu1q((void *)p, 0);
                v8u16 s16  = _mx128_dotpu_h(v, unit8);
                acc32 = _mx128_daddu_w(acc32, s16, unit16);
            }

            v2u64 sum64 = _mx128_dotpu_d(acc32, unit32);
            uint64_t *p64 = (uint64_t *)&sum64;

            dst[by * grid_w + bx]     = (uint8_t)(p64[0] / 64);
            dst[by * grid_w + bx + 1] = (uint8_t)(p64[1] / 64);
        }
    }
}

void bin_16x16(const uint8_t *src, size_t width, size_t height, uint8_t *dst) {
    size_t bx, by, row;
    size_t grid_w = width / 16;   /* 40 */
    size_t grid_h = height / 16;  /* 22  */

    v16u8 unit8 = (v16u8)_mx128_li_b(1);
    v8u16 unit16 = (v8u16)_mx128_li_h(1);
    v4u32 unit32 = (v4u32)_mx128_li_w(1);

    for (by = 0; by < grid_h; by++) {
        for (bx = 0; bx < grid_w; bx++) {
            v4u32 acc32 = (v4u32)_mx128_li_w(0);

            for (row = 0; row < 16; row++) {
                const uint8_t *p = src + (by * 16 + row + 4) * width + bx * 16;
                v16u8 v   = (v16u8)_mx128_lu1q((void *)p, 0);
                v8u16 s16 = _mx128_dotpu_h(v, unit8);
                acc32 = _mx128_daddu_w(acc32, s16, unit16);
            }

            v2u64 sum64 = _mx128_dotpu_d(acc32, unit32);
            uint64_t *p64 = (uint64_t *)&sum64;

            dst[by * grid_w + bx] = (uint8_t)((p64[0] + p64[1]) / 256);
        }
    }
}

void add_u8_to_u16(const uint8_t *src1, const uint8_t *src2, uint16_t *dst, size_t n) {
    size_t i;
    v16u8 zero = (v16u8)_mx128_li_b(0);
    v16i8 idx_lo = {0, 16, 1, 16, 2, 16, 3, 16, 4, 16, 5, 16, 6, 16, 7, 16};
    v16i8 idx_hi = {8, 16, 9, 16,10, 16,11, 16,12, 16,13, 16,14, 16,15, 16};

    for (i = 0; i + 16 <= n; i += 16) {
        v16u8 a = (v16u8)_mx128_lu1q((void *)(src1 + i), 0);
        v16u8 b = (v16u8)_mx128_lu1q((void *)(src2 + i), 0);

        v8u16 a_lo = (v8u16)_mx128_shufv((v16i8)a, (v16i8)zero, idx_lo);
        v8u16 a_hi = (v8u16)_mx128_shufv((v16i8)a, (v16i8)zero, idx_hi);
        v8u16 b_lo = (v8u16)_mx128_shufv((v16i8)b, (v16i8)zero, idx_lo);
        v8u16 b_hi = (v8u16)_mx128_shufv((v16i8)b, (v16i8)zero, idx_hi);

        v8u16 sum_lo = (v8u16)_mx128_adduu_h(a_lo, b_lo);
        v8u16 sum_hi = (v8u16)_mx128_adduu_h(a_hi, b_hi);
        _mx128_su1q((v16i8)sum_lo, (void *)(dst + i),     0);
        _mx128_su1q((v16i8)sum_hi, (void *)(dst + i + 8), 0);
    }
    for (; i < n; i++) {
        dst[i] = (uint16_t)src1[i] + (uint16_t)src2[i];
    }
}

void stack_init(const uint8_t *src, uint16_t *acc, size_t n) {
    size_t i;
    v16i8 idx_lo = {0,16,1,16,2,16,3,16,4,16,5,16,6,16,7,16};
    v16i8 idx_hi = {8,16,9,16,10,16,11,16,12,16,13,16,14,16,15,16};
    v16u8 zero   = (v16u8)_mx128_li_b(0);
    for (i = 0; i + 16 <= n; i += 16) {
        v16u8 v = (v16u8)_mx128_lu1q((void *)(src + i), 0);
        v8u16 lo = (v8u16)_mx128_shufv((v16i8)v, (v16i8)zero, idx_lo);
        v8u16 hi = (v8u16)_mx128_shufv((v16i8)v, (v16i8)zero, idx_hi);
        _mx128_su1q((v16i8)lo, (void *)(acc + i),     0);
        _mx128_su1q((v16i8)hi, (void *)(acc + i + 8), 0);
    }
}

void stack_add(const uint8_t *src, uint16_t *acc, size_t n) {
    /* Avoid shufv (observed to mis-permute on this MXU2 toolchain). Widen
     * u8 → u16 via a stack buffer, then SIMD-add into the u16 accumulator. */
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        uint16_t buf[16] __attribute__((aligned(16)));
        buf[0]  = src[i + 0];  buf[1]  = src[i + 1];
        buf[2]  = src[i + 2];  buf[3]  = src[i + 3];
        buf[4]  = src[i + 4];  buf[5]  = src[i + 5];
        buf[6]  = src[i + 6];  buf[7]  = src[i + 7];
        buf[8]  = src[i + 8];  buf[9]  = src[i + 9];
        buf[10] = src[i + 10]; buf[11] = src[i + 11];
        buf[12] = src[i + 12]; buf[13] = src[i + 13];
        buf[14] = src[i + 14]; buf[15] = src[i + 15];

        v8u16 v_lo = (v8u16)_mx128_lu1q((void *)&buf[0], 0);
        v8u16 v_hi = (v8u16)_mx128_lu1q((void *)&buf[8], 0);
        v8u16 a_lo = (v8u16)_mx128_lu1q((void *)(acc + i),     0);
        v8u16 a_hi = (v8u16)_mx128_lu1q((void *)(acc + i + 8), 0);
        _mx128_su1q((v16i8)_mx128_adduu_h(a_lo, v_lo), (void *)(acc + i),     0);
        _mx128_su1q((v16i8)_mx128_adduu_h(a_hi, v_hi), (void *)(acc + i + 8), 0);
    }
}

void stack_add_sq(const uint8_t *src, uint32_t *acc, size_t n) {
    /* Avoid shufv-based widening (broken on this MXU2 toolchain). Square each
     * pixel in scalar (u8² ≤ 65025 fits in u32) and SIMD-add into u32 acc. */
    size_t i;
    for (i = 0; i + 16 <= n; i += 16) {
        uint32_t sq[16] __attribute__((aligned(16)));
        for (int k = 0; k < 16; k++) {
            uint32_t v = src[i + k];
            sq[k] = v * v;
        }

        v4u32 q0 = (v4u32)_mx128_lu1q((void *)&sq[0],  0);
        v4u32 q1 = (v4u32)_mx128_lu1q((void *)&sq[4],  0);
        v4u32 q2 = (v4u32)_mx128_lu1q((void *)&sq[8],  0);
        v4u32 q3 = (v4u32)_mx128_lu1q((void *)&sq[12], 0);

        v4u32 a0 = (v4u32)_mx128_lu1q((void *)(acc + i),      0);
        v4u32 a1 = (v4u32)_mx128_lu1q((void *)(acc + i + 4),  0);
        v4u32 a2 = (v4u32)_mx128_lu1q((void *)(acc + i + 8),  0);
        v4u32 a3 = (v4u32)_mx128_lu1q((void *)(acc + i + 12), 0);

        _mx128_su1q((v16i8)_mx128_adduu_w(a0, q0), (void *)(acc + i),      0);
        _mx128_su1q((v16i8)_mx128_adduu_w(a1, q1), (void *)(acc + i + 4),  0);
        _mx128_su1q((v16i8)_mx128_adduu_w(a2, q2), (void *)(acc + i + 8),  0);
        _mx128_su1q((v16i8)_mx128_adduu_w(a3, q3), (void *)(acc + i + 12), 0);
    }
}

void sub_bin_8x8(const uint8_t *cur, const uint8_t *prev,
                 size_t width, size_t height, uint8_t *dst) {
    size_t bx, by, row;
    size_t grid_w = width / 8;   /* 80 */
    size_t grid_h = height / 8;  /* 45 */
 
    v16u8 unit8  = (v16u8)_mx128_li_b(1);
    v8u16 unit16 = (v8u16)_mx128_li_h(1);
    v4u32 unit32 = (v4u32)_mx128_li_w(1);
 
    for (by = 0; by < grid_h; by++) {
        for (bx = 0; bx < grid_w; bx += 2) {
            /* acc32 のレーン構成: dotpu の縮約により
               [cell0 半分, cell0 半分, cell1 半分, cell1 半分] 相当の
               部分和が並ぶ。最後に dotpu_d で 64bit x2 (= セル毎) に縮約。 */
            v4u32 acc32 = (v4u32)_mx128_li_w(0);
 
            for (row = 0; row < 8; row++) {
                const uint8_t *pc = cur  + (by * 8 + row) * width + bx * 8;
                const uint8_t *pp = prev + (by * 8 + row) * width + bx * 8;
                v16u8 c = (v16u8)_mx128_lu1q((void *)pc, 0);
                v16u8 p = (v16u8)_mx128_lu1q((void *)pp, 0);
                /* 飽和減算: 増光方向のみ拾う (sub_saturate と同一の意味) */
                v16u8 d = (v16u8)_mx128_subuu_b(c, p);
                v8u16 s16 = _mx128_dotpu_h(d, unit8);
                acc32 = _mx128_daddu_w(acc32, s16, unit16);
            }
 
            v2u64 sum64 = _mx128_dotpu_d(acc32, unit32);
            uint64_t *p64 = (uint64_t *)&sum64;
 
            dst[by * grid_w + bx]     = (uint8_t)(p64[0] / 64);
            dst[by * grid_w + bx + 1] = (uint8_t)(p64[1] / 64);
        }
    }
}