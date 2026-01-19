#!/bin/bash

set -ex

### Build raw images by proprocessing in chroot.
### Prefer using cloud-init for customization, and modify here only when required.

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

remove_packages() {
	apt purge -y linux-image-*
}

# cloud-init requires several minutes of extra delay for installing packages,
# Preinstall required packages here to reduce initial booting time.
install_packages() {
	# Prevent apt from cleaning up the cache
	echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/01keep-debs

	# Speed up dpkg by excluding unnecessary files
	# This significantly reduces unpacking time and final image size
	mkdir -p /etc/dpkg/dpkg.cfg.d
	cat <<EOF > /etc/dpkg/dpkg.cfg.d/99-speedup
path-exclude=/usr/share/doc/*
path-exclude=/usr/share/man/*
path-exclude=/usr/share/locale/*
path-exclude=/usr/share/info/*
path-exclude=/usr/share/lintian/*
EOF

	# Prevent services from starting during package installation
	echo -e '#!/bin/sh\nexit 101' > /usr/sbin/policy-rc.d
	chmod +x /usr/sbin/policy-rc.d

	INSTALL_PACKAGES=(
		kmod
		udev
		avahi-daemon
		avahi-utils
		libbpf-tools
		libnss-mdns
		libvulkan1
		mesa-vulkan-drivers
		procps
		pulseaudio
		systemd-zram-generator
		vulkan-tools
		weston
		xwayland
	)

	apt update || apt update
	DEBIAN_FRONTEND=noninteractive apt install -y eatmydata
	DEBIAN_FRONTEND=noninteractive eatmydata apt -o Dpkg::Options::="--force-confdef" -o Dpkg::Options::="--force-confold" upgrade -y
	eatmydata apt install --no-install-recommends -y "${INSTALL_PACKAGES[@]}"
}

# TODO: Install ttyd from debian package after it picks up our patches
install_ttyd() {
	cp --preserve=mode -vpR /mnt/ttyd/* /
}

modify_pre_cloud_init_configs() {
	# Since we boot directly with custom kernel, we wouldn't need EFI partition.
	# However, boot failed because of the error from /etc/fstab, so fix here.
	sed -i '\|/boot/efi|s|^|#|' /etc/fstab

	# LLMNR cause port notification (5355) before cloud-init can configure.
	# TODO: Move this to cloud-init, and add the port to the disallow-list.
	sed -i 's/#LLMNR=yes/LLMNR=no/' /etc/systemd/resolved.conf

	# Disable systemd-networkd-wait-online.service. Not only it randomly hangs
	# for 2 minutes, but also it slows down the boot process even though this
	# system doesn't need networking.
	systemctl disable systemd-networkd-wait-online.service
	systemctl mask systemd-networkd-wait-online.service

	# Cleanup build-time tools
	DEBIAN_FRONTEND=noninteractive apt purge -y eatmydata

	# Cleanup build-time optimizations
	rm -f /etc/apt/apt.conf.d/01keep-debs
	rm -f /usr/sbin/policy-rc.d
	rm -f /etc/dpkg/dpkg.cfg.d/99-speedup
}

remove_packages
install_packages
install_ttyd
modify_pre_cloud_init_configs
