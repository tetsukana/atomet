/*
 * atomet.c - Atomet ISP control module
 *
 * Provides ioctl interface for AE control and ISP bypass configuration.
 * Linked into tx_isp_t31.ko via libt31-firmware.a for direct symbol access.
 *
 * Usage from userspace:
 *   int fd = open("/dev/atomet", O_RDWR);
 *   struct ae_params ae;
 *   ioctl(fd, ATOMET_GET_AE_ATTR, &ae);
 *   ae.it_manual_en = 1;
 *   ae.it_value = 500;
 *   ioctl(fd, ATOMET_SET_AE_ATTR, &ae);
 */
#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/uaccess.h>
#include <linux/string.h>

#include "atomet.h"

/* Symbols from libt31-firmware.a */
extern struct ae_params tisp_ae_ctrls;
extern int tisp_day_or_night_s_ctrl(int mode);
extern int day_night;
extern void *tparams_night;
extern void *tparams_day;

/* tparams_dst is not exported by name - resolve from tparams_night at runtime.
 * After dn_ctrl(1), tparams_dst contains a copy of tparams_night.
 * We modify tparams_night directly then call dn_ctrl(1) to apply. */

/* ---- AE control ---- */

static int atomet_get_ae(unsigned long arg)
{
    if (copy_to_user((void __user *)arg, &tisp_ae_ctrls,
                     sizeof(struct ae_params)))
        return -EFAULT;
    return 0;
}

static int atomet_set_ae(unsigned long arg)
{
    struct ae_params params;

    if (copy_from_user(&params, (void __user *)arg,
                       sizeof(struct ae_params)))
        return -EFAULT;

    memcpy(&tisp_ae_ctrls, &params, sizeof(struct ae_params));
    return 0;
}

/* ---- TOP_BYPASS control ---- */

static int atomet_get_bypass(unsigned long arg)
{
    struct top_bypass bp;

    if (!tparams_night)
        return -ENODEV;

    memcpy(&bp, tparams_night, sizeof(struct top_bypass));

    if (copy_to_user((void __user *)arg, &bp, sizeof(bp)))
        return -EFAULT;
    return 0;
}

static int atomet_set_bypass(unsigned long arg)
{
    struct top_bypass bp;

    if (!tparams_night)
        return -ENODEV;

    if (copy_from_user(&bp, (void __user *)arg, sizeof(bp)))
        return -EFAULT;

    /* Write to tparams_night, then apply via night mode */
    memcpy(tparams_night, &bp, sizeof(struct top_bypass));
    tisp_day_or_night_s_ctrl(1);

    return 0;
}

/* ---- Day/Night switch ---- */

static int atomet_set_day_night(unsigned long arg)
{
    int mode;

    if (copy_from_user(&mode, (void __user *)arg, sizeof(int)))
        return -EFAULT;

    if (mode < 0 || mode > 1)
        return -EINVAL;

    tisp_day_or_night_s_ctrl(mode);
    return 0;
}

/* ---- ISP digital gain cap ---- */

static int atomet_set_max_idg(unsigned long arg)
{
    uint32_t val;

    if (copy_from_user(&val, (void __user *)arg, sizeof(val)))
        return -EFAULT;

    tisp_ae_ctrls.max_idg = val;
    return 0;
}

/* ---- Freeze / Unfreeze AE ---- */

static int atomet_freeze_ae(void)
{
    tisp_ae_ctrls.ae_mode = 1;
    tisp_ae_ctrls.it_manual_en = 1;
    tisp_ae_ctrls.ag_manual_en = 1;
    tisp_ae_ctrls.dg_manual_en = 1;
    return 0;
}

static int atomet_unfreeze_ae(void)
{
    tisp_ae_ctrls.ae_mode = 0;
    tisp_ae_ctrls.it_manual_en = 0;
    tisp_ae_ctrls.ag_manual_en = 0;
    tisp_ae_ctrls.dg_manual_en = 0;
    return 0;
}

/* ---- ioctl dispatch ---- */

static long atomet_ioctl(struct file *filp, unsigned int cmd,
                         unsigned long arg)
{
    switch (cmd) {
    case ATOMET_GET_AE_ATTR:
        return atomet_get_ae(arg);
    case ATOMET_SET_AE_ATTR:
        return atomet_set_ae(arg);
    case ATOMET_GET_TOP_BYPASS:
        return atomet_get_bypass(arg);
    case ATOMET_SET_TOP_BYPASS:
        return atomet_set_bypass(arg);
    case ATOMET_SET_DAY_NIGHT:
        return atomet_set_day_night(arg);
    case ATOMET_FREEZE_AE:
        return atomet_freeze_ae();
    case ATOMET_UNFREEZE_AE:
        return atomet_unfreeze_ae();
    case ATOMET_SET_MAX_IDG:
        return atomet_set_max_idg(arg);
    default:
        return -ENOTTY;
    }
}

/* ---- file operations ---- */

static int atomet_open(struct inode *inode, struct file *filp)
{
    return 0;
}

static int atomet_release(struct inode *inode, struct file *filp)
{
    return 0;
}

static const struct file_operations atomet_fops = {
    .owner          = THIS_MODULE,
    .unlocked_ioctl = atomet_ioctl,
    .open           = atomet_open,
    .release        = atomet_release,
};

static struct miscdevice atomet_dev = {
    .minor = MISC_DYNAMIC_MINOR,
    .name  = "atomet",
    .fops  = &atomet_fops,
};

/* ---- init/exit ---- */

int atomet_init(void)
{
    int ret;

    ret = misc_register(&atomet_dev);
    if (ret) {
        printk(KERN_ERR "atomet: misc_register failed: %d\n", ret);
        return ret;
    }

    printk(KERN_INFO "atomet: /dev/atomet ready\n");
    printk(KERN_INFO "  ae_ctrls   @ %px\n", &tisp_ae_ctrls);
    printk(KERN_INFO "  tp_night   @ %px\n", tparams_night);
    printk(KERN_INFO "  tp_day     @ %px\n", tparams_day);
    printk(KERN_INFO "  day_night  = %d\n", day_night);


    return 0;
}

void atomet_exit(void)
{
    misc_deregister(&atomet_dev);
    printk(KERN_INFO "atomet: removed\n");
}