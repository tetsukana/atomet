#pragma once

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <math.h>
#include <mxu2.h>

#define VECTOR_SIZE 16

void add_saturate(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n);
void sub_saturate(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n);
void absdiff(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n);
void brightest(const uint8_t *src1, const uint8_t *src2, uint8_t *dst, size_t n);
void mean_stddev(const uint8_t *src, size_t n, double *mean, double *stddev);
void bin_8x8(const uint8_t *src, size_t width, size_t height, uint8_t *dst);
void bin_16x16(const uint8_t *src, size_t width, size_t height, uint8_t *dst);
void stack_init(const uint8_t *src, uint16_t *acc, size_t n);
void stack_add(const uint8_t *src, uint16_t *acc, size_t n);

/* Star extraction (morphological peak detection + voting) */
void stars_process_row(const uint8_t *prev, const uint8_t *curr, const uint8_t *next,
                       uint8_t *vote_row, uint8_t threshold);
void stars_calibrate_row(const uint8_t *prev, const uint8_t *curr, const uint8_t *next,
                         uint32_t *hist);