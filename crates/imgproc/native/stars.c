#include "imgproc.h"
#include <string.h>

/* ================================================================
   Star extraction — MXU2 SIMD morphological peak detection + voting
   ================================================================

   Core kernel: for each pixel in a row, using a 3×3 window:
     1. 3×3 max  → is_peak = (center == max3x3)
     2. ring_min  = min of 8 neighbours (excluding center)
     3. contrast  = center - ring_min  (saturating)
     4. vote if is_peak AND contrast > threshold
   ================================================================ */

#define STAR_WIDTH 640

/* Process one row of 640 pixels using MXU2 SIMD (16 px per iteration).
 *
 * prev/curr/next point to row buffers with 1-byte padding on each side,
 * i.e. prev = &rows[(y-1)%3][1], so prev[-1] and prev[640] are valid.
 *
 * vote_row = &vote_map[y * 640]
 * thresh_vec = threshold broadcast to all 16 bytes
 */
void stars_process_row(
    const uint8_t *prev,
    const uint8_t *curr,
    const uint8_t *next,
    uint8_t       *vote_row,
    uint8_t        threshold
) {
    uint8_t tbuf[16];
    memset(tbuf, threshold, 16);
    v16u8 thresh_vec = (v16u8)_mx128_lu1q((void*)tbuf, 0);
    int x;

    for (x = 0; x < STAR_WIDTH; x += 16) {
        /* Load 3 shifts for each of 3 rows (unaligned) */
        v16u8 p_L = (v16u8)_mx128_lu1q((void*)(prev + x - 1), 0);
        v16u8 p_C = (v16u8)_mx128_lu1q((void*)(prev + x    ), 0);
        v16u8 p_R = (v16u8)_mx128_lu1q((void*)(prev + x + 1), 0);

        v16u8 c_L = (v16u8)_mx128_lu1q((void*)(curr + x - 1), 0);
        v16u8 c_C = (v16u8)_mx128_lu1q((void*)(curr + x    ), 0);
        v16u8 c_R = (v16u8)_mx128_lu1q((void*)(curr + x + 1), 0);

        v16u8 n_L = (v16u8)_mx128_lu1q((void*)(next + x - 1), 0);
        v16u8 n_C = (v16u8)_mx128_lu1q((void*)(next + x    ), 0);
        v16u8 n_R = (v16u8)_mx128_lu1q((void*)(next + x + 1), 0);

        /* 3×3 max */
        v16u8 hmax_p = _mx128_maxu_b(_mx128_maxu_b(p_L, p_C), p_R);
        v16u8 hmax_c = _mx128_maxu_b(_mx128_maxu_b(c_L, c_C), c_R);
        v16u8 hmax_n = _mx128_maxu_b(_mx128_maxu_b(n_L, n_C), n_R);
        v16u8 max3x3 = _mx128_maxu_b(_mx128_maxu_b(hmax_p, hmax_c), hmax_n);

        /* is_peak: center == max3x3 → 0xFF per lane, else 0x00 */
        v16i8 is_peak = _mx128_ceq_b((v16i8)c_C, (v16i8)max3x3);

        /* ring_min: min of 8 neighbours (skip center) */
        v16u8 ring = _mx128_minu_b(p_L, p_C);
        ring = _mx128_minu_b(ring, p_R);
        ring = _mx128_minu_b(ring, c_L);
        /* skip c_C */
        ring = _mx128_minu_b(ring, c_R);
        ring = _mx128_minu_b(ring, n_L);
        ring = _mx128_minu_b(ring, n_C);
        ring = _mx128_minu_b(ring, n_R);

        /* contrast = center - ring_min (saturating) */
        v16u8 contrast = (v16u8)_mx128_subus_b(c_C, ring);

        /* contrast > threshold  ⟺  threshold < contrast */
        v16i8 above = _mx128_cltu_b(thresh_vec, contrast);

        /* Both conditions: is_peak AND above_threshold */
        v16i8 mask = _mx128_andv(is_peak, above);

        /* 0xFF → 0x01 */
        v16i8 ones = _mx128_andib(mask, 1);

        /* Accumulate into vote_map (saturating add) */
        v16u8 vmap = (v16u8)_mx128_lu1q((void*)(vote_row + x), 0);
        vmap = _mx128_adduu_b(vmap, (v16u8)ones);
        _mx128_su1q((v16i8)vmap, (void*)(vote_row + x), 0);
    }
}

/* Calibrate threshold from a single row's contrast values.
 * Same 3×3 kernel but instead of voting, accumulates a contrast histogram.
 * hist must be a uint32_t[256] array (caller manages accumulation across rows/frames).
 */
void stars_calibrate_row(
    const uint8_t *prev,
    const uint8_t *curr,
    const uint8_t *next,
    uint32_t      *hist
) {
    int x;
    for (x = 0; x < STAR_WIDTH; x += 16) {
        v16u8 p_L = (v16u8)_mx128_lu1q((void*)(prev + x - 1), 0);
        v16u8 p_C = (v16u8)_mx128_lu1q((void*)(prev + x    ), 0);
        v16u8 p_R = (v16u8)_mx128_lu1q((void*)(prev + x + 1), 0);

        v16u8 c_L = (v16u8)_mx128_lu1q((void*)(curr + x - 1), 0);
        v16u8 c_C = (v16u8)_mx128_lu1q((void*)(curr + x    ), 0);
        v16u8 c_R = (v16u8)_mx128_lu1q((void*)(curr + x + 1), 0);

        v16u8 n_L = (v16u8)_mx128_lu1q((void*)(next + x - 1), 0);
        v16u8 n_C = (v16u8)_mx128_lu1q((void*)(next + x    ), 0);
        v16u8 n_R = (v16u8)_mx128_lu1q((void*)(next + x + 1), 0);

        /* ring_min */
        v16u8 ring = _mx128_minu_b(p_L, p_C);
        ring = _mx128_minu_b(ring, p_R);
        ring = _mx128_minu_b(ring, c_L);
        ring = _mx128_minu_b(ring, c_R);
        ring = _mx128_minu_b(ring, n_L);
        ring = _mx128_minu_b(ring, n_C);
        ring = _mx128_minu_b(ring, n_R);

        /* contrast = center - ring_min */
        v16u8 contrast = (v16u8)_mx128_subus_b(c_C, ring);

        /* Extract 16 bytes to scalar and accumulate histogram */
        uint8_t buf[16];
        _mx128_su1q((v16i8)contrast, (void*)buf, 0);
        int i;
        for (i = 0; i < 16; i++) {
            hist[buf[i]]++;
        }
    }
}
