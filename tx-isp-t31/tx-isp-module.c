/*
 * Video Class definitions of Tomahawk series SoC.
 *
 * Copyright 2017, <xianghui.shen@ingenic.com>
 *
 * This program is licensed "as is" without any
 * warranty of any kind, whether express or implied.
 */

#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/version.h>

extern int tx_isp_init(void);
extern void tx_isp_exit(void);

extern int atomet_init(void);
extern void atomet_exit(void);

static int __init tx_isp_module_init(void) {
    int ret = tx_isp_init();
    if (ret) return ret;
    atomet_init();
    return 0;
}

static void __exit tx_isp_module_exit(void) {
    atomet_exit();
    tx_isp_exit();
}

module_init(tx_isp_module_init);
module_exit(tx_isp_module_exit);

MODULE_AUTHOR("Ingenic xhshen");
MODULE_DESCRIPTION("tx isp driver");
MODULE_LICENSE("GPL");
