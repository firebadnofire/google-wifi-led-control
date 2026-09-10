include $(TOPDIR)/rules.mk

PKG_NAME:=gale-led
PKG_VERSION:=1.1.1
PKG_RELEASE:=1

PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=firebadnofire
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)
PKG_BUILD_PARALLEL:=1

include $(INCLUDE_DIR)/package.mk

RUST_TARGET:=armv7-unknown-linux-musleabihf

define Package/gale-led
  SECTION:=utils
  CATEGORY:=Utilities
  SUBMENU:=LEDs
  TITLE:=Google Wifi Gale RGB LED control utility
  URL:=https://github.com/firebadnofire/google-wifi-led-control
  DEPENDS:=@TARGET_ipq40xx_chromium
endef

define Package/gale-led/description
 A small one-shot utility and init service for controlling the built-in RGB
 status LED on Google Wifi Gale hardware.
endef

define Package/gale-led/conffiles
/etc/config/gale-led
endef

define Package/luci-app-gale-led
  SECTION:=luci
  CATEGORY:=LuCI
  SUBMENU:=3. Applications
  TITLE:=LuCI support for Google Wifi Gale RGB LED control
  URL:=https://github.com/firebadnofire/google-wifi-led-control
  PKGARCH:=all
  DEPENDS:=+gale-led +luci-base
endef

define Package/luci-app-gale-led/description
 Modern LuCI JavaScript interface and narrowly scoped rpcd/ucode backend for
 gale-led.
endef

define Build/Prepare
	$(RM) -r $(PKG_BUILD_DIR)
	$(INSTALL_DIR) $(PKG_BUILD_DIR)/src
	$(CP) $(CURDIR)/Cargo.toml $(CURDIR)/Cargo.lock $(CURDIR)/LICENSE $(PKG_BUILD_DIR)/
	$(CP) $(CURDIR)/src/*.rs $(PKG_BUILD_DIR)/src/
endef

define Build/Compile
	(cd $(PKG_BUILD_DIR) && \
		CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$(TARGET_CC_NOCACHE)" \
		RUSTFLAGS="-C target-feature=-crt-static -C link-arg=-Wl,-z,now -C link-arg=-Wl,-z,relro" \
		cargo build --release --locked --target $(RUST_TARGET))
endef

define Package/gale-led/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/target/$(RUST_TARGET)/release/gale-led $(1)/usr/bin/gale-led
	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) $(CURDIR)/files/gale-led.config $(1)/etc/config/gale-led
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) $(CURDIR)/files/gale-led.init $(1)/etc/init.d/gale-led
endef

define Package/luci-app-gale-led/install
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/system
	$(INSTALL_DATA) $(CURDIR)/luci/htdocs/luci-static/resources/view/system/gale-led.js \
		$(1)/www/luci-static/resources/view/system/gale-led.js
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) $(CURDIR)/luci/root/usr/share/luci/menu.d/luci-app-gale-led.json \
		$(1)/usr/share/luci/menu.d/luci-app-gale-led.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) $(CURDIR)/luci/root/usr/share/rpcd/acl.d/luci-app-gale-led.json \
		$(1)/usr/share/rpcd/acl.d/luci-app-gale-led.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) $(CURDIR)/luci/root/usr/share/rpcd/ucode/gale-led \
		$(1)/usr/share/rpcd/ucode/gale-led
endef

$(eval $(call BuildPackage,gale-led))
$(eval $(call BuildPackage,luci-app-gale-led))
