#!/bin/bash

set -ex

SCRIPT_PATH=$(readlink -f -- "${0}")
SCRIPT_DIR=$(dirname -- ${SCRIPT_PATH});

LOCALDEBS="${SCRIPT_DIR}/localdebs"
LOCALFILES="${SCRIPT_DIR}/files"

install_localdebs() {
	dpkg -i "${LOCALDEBS}"/*.deb
}

_update_datetime() {
	timedatectl set-ntp true
	until timedatectl status | grep 'System clock synchronized: yes'; do
		sleep 3
	done
}

# TODO: Bundle these to rootfs
install_deps() {
	# This is prerequisite of `apt update`.
	_update_datetime

	INSTALL_PACKAGES=(
		kmod
		udev
		avahi-daemon
		avahi-utils
		bpfcc-tools
		libnss-mdns
		procps
		pulseaudio
		systemd-zram-generator
	)

	INSTALL_PACKAGES_BOOKWORM_BACKPORTS=(
		weston
		xwayland
		mesa-vulkan-drivers
		libvulkan1
		vulkan-tools
	)
	apt update
	apt upgrade -y
	apt install --no-install-recommends -y "${INSTALL_PACKAGES[@]}"
	apt install --no-install-recommends -t bookworm-backports -y "${INSTALL_PACKAGES_BOOKWORM_BACKPORTS[@]}"
}

_copy_files() {
	cp -v -R "${LOCALFILES}"/* /
}

_modify_configs() {
	sed -i 's/#LLMNR=yes/LLMNR=no/' /etc/systemd/resolved.conf
}

_restart_services() {
	CONFIG_CHANGED_SERVICES=(
		avahi_ttyd.service
		backup_mount.service
		virtiofs.service
		virtiofs_internal.service
		ttyd.service
	)
	systemctl enable --now "${CONFIG_CHANGED_SERVICES[@]}"
}

apply_avf_configs() {
	_copy_files
	_modify_configs
	_restart_services
}

install_localdebs
install_deps
apply_avf_configs
