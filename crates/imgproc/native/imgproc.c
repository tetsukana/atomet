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
    size_t i;
    v16i8 idx_lo = {0,16,1,16,2,16,3,16,4,16,5,16,6,16,7,16};
    v16i8 idx_hi = {8,16,9,16,10,16,11,16,12,16,13,16,14,16,15,16};
    v16u8 zero   = (v16u8)_mx128_li_b(0);
    for (i = 0; i + 16 <= n; i += 16) {
        v16u8 v    = (v16u8)_mx128_lu1q((void *)(src + i), 0);
        v8u16 v_lo = (v8u16)_mx128_shufv((v16i8)v, (v16i8)zero, idx_lo);
        v8u16 v_hi = (v8u16)_mx128_shufv((v16i8)v, (v16i8)zero, idx_hi);
        v8u16 a_lo = (v8u16)_mx128_lu1q((void *)(acc + i),     0);
        v8u16 a_hi = (v8u16)_mx128_lu1q((void *)(acc + i + 8), 0);
        _mx128_su1q((v16i8)_mx128_adduu_h(a_lo, v_lo), (void *)(acc + i),     0);
        _mx128_su1q((v16i8)_mx128_adduu_h(a_hi, v_hi), (void *)(acc + i + 8), 0);
    }
}