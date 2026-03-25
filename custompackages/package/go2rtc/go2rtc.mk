################################################################################
#
# go2rtc prebuilt binary
#
################################################################################

GO2RTC_VERSION = 1.9.14
GO2RTC_SITE = https://github.com/AlexxIT/go2rtc/releases/download/v$(GO2RTC_VERSION)
GO2RTC_SOURCE = go2rtc_linux_mipsel
GO2RTC_SITE_METHOD = wget
GO2RTC_INSTALL_TARGET = YES

GO2RTC_EXTRACT_CMDS = true

define GO2RTC_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(GO2RTC_DL_DIR)/$(GO2RTC_SOURCE) $(TARGET_DIR)/usr/bin/go2rtc
endef

$(eval $(generic-package))