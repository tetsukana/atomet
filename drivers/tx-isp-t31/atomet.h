/*
 * atomet.h - Atomet ISP control interface
 *
 * Shared between kernel module and userspace applications.
 * Structures reverse-engineered from tx_isp_t31.ko using Ghidra.
 */
#ifndef ATOMET_H_
#define ATOMET_H_

#ifdef __KERNEL__
#include <linux/types.h>
#include <linux/ioctl.h>
#else
#include <stdint.h>
#include <sys/ioctl.h>
#endif

/*
 * AE control parameters - tisp_ae_ctrls
 * Reverse-engineered from tx_isp_t31.ko using Ghidra.
 * ELF symbol: tisp_ae_ctrls, GLOBAL, .bss, size 152 (0x98)
 *
 * ae_mode=1: full manual bypass (tisp_ae_manual_set writes all values directly)
 * it/ag/dg_manual_en=1: freeze individual param, AE adjusts the rest
 * s_* fields: short frame exposure for WDR mode
 */
struct ae_params {
    uint32_t ae_mode;           /* +0x00 0=auto, 1=full manual bypass */
    uint32_t ag_value;          /* +0x04 analog gain (sensor) */
    uint32_t sdg_value;         /* +0x08 sensor digital gain */
    uint32_t it_value;          /* +0x0c integration time (lines) */
    uint32_t idg_value;         /* +0x10 ISP digital gain */
    uint32_t max_ag;            /* +0x14 */
    uint32_t max_sdg;           /* +0x18 */
    uint32_t max_it;            /* +0x1c */
    uint32_t max_idg;           /* +0x20 */
    uint32_t total_ev;          /* +0x24 read-only */
    uint32_t total_gain_log2;   /* +0x28 read-only */
    uint32_t again_log2;        /* +0x2c read-only */
    uint32_t _pad;              /* +0x30 unknown */
    uint32_t it_manual_en;      /* +0x34 freeze IT only, AE adjusts AG/DG */
    uint32_t ag_manual_en;      /* +0x38 freeze AG only */
    uint32_t dg_manual_en;      /* +0x3c freeze DG only */
    uint32_t s_it_en;           /* +0x40 short frame IT enable (WDR) */
    uint32_t s_ag_en;           /* +0x44 short frame AG enable (WDR) */
    uint32_t s_ag_val;          /* +0x48 */
    uint32_t s_it_val;          /* +0x4c */
    uint32_t max_ag_short;      /* +0x50 */
    uint32_t max_it_short;      /* +0x54 */
    uint32_t max_sdg_short;     /* +0x58 */
    uint32_t max_idg_short;     /* +0x5c */
    uint32_t s_freeze;          /* +0x60 */
    uint32_t s_sdg;             /* +0x64 */
    uint32_t s_sdg_manual;      /* +0x68 */
    uint32_t s_idg;             /* +0x6c */
    uint32_t min_ag;            /* +0x70 */
    uint32_t min_sdg;           /* +0x74 */
    uint32_t min_it;            /* +0x78 */
    uint32_t min_idg;           /* +0x7c */
    uint32_t min_ag_short;      /* +0x80 */
    uint32_t min_it_short;      /* +0x84 */
    uint32_t min_sdg_short;     /* +0x88 */
    uint32_t min_idg_short;     /* +0x8c */
    uint32_t sdg_en;            /* +0x90 */
    uint32_t s_sdg_en;          /* +0x94 */
};  /* 152 bytes (0x98) */

/*
 * TOP_BYPASS register - tparams_dst[0..31]
 * Each uint32_t is 0 or 1 (one bit per ISP module).
 * Stored LSB-first: bits[0] = tparams_dst[0], bits[31] = tparams_dst[31].
 * Written to ISP register 0x0c after mask/force by tisp_day_or_night_s_ctrl.
 *
 * Bit mapping (partially known from Day/Night diff):
 *   bit 0:  1=bypass (unknown module)
 *   bit 2:  Night=1, Day=0  (unknown module)
 *   bit 3:  1=bypass (unknown module)
 *   bit 5:  Night=1, Day=0  (unknown module)
 *   bit 6:  Day=1, Night=0  (unknown module)
 * TODO: complete bit-to-module mapping
 */
struct top_bypass {
    uint32_t bits[32];
};

/* ioctl interface */
#define ATOMET_IOC_MAGIC 'M'

#define ATOMET_GET_AE_ATTR      _IOR(ATOMET_IOC_MAGIC, 0, struct ae_params)
#define ATOMET_SET_AE_ATTR      _IOW(ATOMET_IOC_MAGIC, 1, struct ae_params)
#define ATOMET_GET_TOP_BYPASS   _IOR(ATOMET_IOC_MAGIC, 2, struct top_bypass)
#define ATOMET_SET_TOP_BYPASS   _IOW(ATOMET_IOC_MAGIC, 3, struct top_bypass)
#define ATOMET_SET_DAY_NIGHT    _IOW(ATOMET_IOC_MAGIC, 4, int)
#define ATOMET_FREEZE_AE        _IO(ATOMET_IOC_MAGIC, 5)
#define ATOMET_UNFREEZE_AE      _IO(ATOMET_IOC_MAGIC, 6)
#define ATOMET_SET_MAX_IDG      _IOW(ATOMET_IOC_MAGIC, 7, uint32_t)

#endif /* ATOMET_H_ */