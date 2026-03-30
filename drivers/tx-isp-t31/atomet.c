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
#include <linux/proc_fs.h>
#include <linux/seq_file.h>

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

static char *ntok(char *p, char **e)
{
    while (*p==' '||*p=='\t') p++;
    if (*p=='\0'||*p=='\n') return NULL;
    *e = p;
    while (**e&&**e!=' '&&**e!='\t'&&**e!='\n') (*e)++;
    return p;
}

static int pu(const char *s, int l, unsigned int *o)
{
    unsigned int v=0; int i;
    if (l>=2&&s[0]=='0'&&(s[1]=='x'||s[1]=='X')) {
        for(i=2;i<l;i++){char c=s[i];
            if(c>='0'&&c<='9')v=v*16+(c-'0');
            else if(c>='a'&&c<='f')v=v*16+(c-'a'+10);
            else if(c>='A'&&c<='F')v=v*16+(c-'A'+10);
            else return -1;}
    } else { for(i=0;i<l;i++){if(s[i]<'0'||s[i]>'9')return -1;v=v*10+(s[i]-'0');} }
    *o=v; return 0;
}

static int ae_show(struct seq_file *m, void *v)
{
    seq_printf(m, "Hello World!\n");
    return 0;
}


static int ae_open(struct inode *i, struct file *f)
{ return single_open(f, ae_show, NULL); }

static ssize_t ae_wr(struct file *f, const char __user *buf,
                     size_t cnt, loff_t *pos)
{
    char b[256]; int l; char *p;

    /* Text commands */
    if(cnt>=sizeof(b))cnt=sizeof(b)-1;
    if(copy_from_user(b,buf,cnt))return -EFAULT;
    b[cnt]=0; l=strlen(b); if(l>0&&b[l-1]=='\n')b[--l]=0;
    p=b;

    /* freeze */
    if(strncmp(p,"freeze",6)==0 && (p[6]==0||p[6]==' ')) {
        tisp_ae_ctrls.ae_mode = 1;
        tisp_ae_ctrls.it_manual_en = 1;
        tisp_ae_ctrls.ag_manual_en = 1; 
        tisp_ae_ctrls.dg_manual_en = 1;
        return cnt;
    }

    /* unfreeze */
    if(strncmp(p,"unfreeze",8)==0) {
        tisp_ae_ctrls.ae_mode=0;
        tisp_ae_ctrls.it_manual_en=0;
        tisp_ae_ctrls.ag_manual_en=0;
        tisp_ae_ctrls.dg_manual_en=0;
        return cnt;
    }

    /* set <it> <ag> <dg> */
    if(strncmp(p,"set ",4)==0) {
        unsigned int a,b2,c; char*t1,*e1,*t2,*e2,*t3,*e3;
        p+=4; t1=ntok(p,&e1);t2=ntok(e1,&e2);t3=ntok(e2,&e3);
        if(!t1||!t2||!t3)return cnt;
        if(pu(t1,e1-t1,&a)||pu(t2,e2-t2,&b2)||pu(t3,e3-t3,&c))return cnt;
        tisp_ae_ctrls.ae_mode = 1;
        tisp_ae_ctrls.it_value = a;
        tisp_ae_ctrls.ag_value = b2;
        tisp_ae_ctrls.idg_value = c;
        tisp_ae_ctrls.it_manual_en = 1;
        tisp_ae_ctrls.ag_manual_en = 1; 
        tisp_ae_ctrls.dg_manual_en = 1;
        return cnt;
    }

    return cnt;
}


static const struct file_operations fops = {
    .owner=THIS_MODULE,.open=ae_open,.read=seq_read,
    .write=ae_wr,.llseek=seq_lseek,.release=single_release,
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

    if (!proc_create("ae_ctrl", 0666, NULL, &fops)) return -ENOMEM;
    
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