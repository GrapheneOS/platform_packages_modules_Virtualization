#!/bin/bash

# This is a script to build a Debian image that can run in a VM created via AVF.
# TODOs:
# - Add Android-specific packages via a new class
# - Use a stable release from debian-cloud-images

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

show_help() {
	echo "Usage: sudo $0 [OPTION]... [FILE]"
	echo "Builds a debian image and save it to FILE. [sudo is required]"
	echo "Options:"
	echo "-h         Print usage and this help message and exit."
	echo "-a ARCH    Architecture of the image [default is host arch: $(uname -m)]"
	echo "-g         Use Debian generic kernel [default is our custom kernel]"
	echo "-r         Release mode build"
	echo "-u         Set VM boot mode to u-boot [default is to load kernel directly]"
	echo "-w         Save temp work directory [for debugging]"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root." ; exit 1
	fi
}

parse_options() {
	while getopts "a:ghruw" option; do
		case ${option} in
			h)
				show_help ; exit
				;;
			a)
				arch="$OPTARG"
				;;
			g)
				use_generic_kernel=1
				;;
			r)
				mode=release
				;;
			u)
				uboot=1
				;;
			w)
				save_workdir=1
				;;
			*)
				echo "Invalid option: $OPTARG" ; exit 1
				;;
		esac
	done
	case "$arch" in
		aarch64)
			debian_arch="arm64"
			;;
		x86_64)
			debian_arch="amd64"
			;;
		*)
			echo "Invalid architecture: $arch" ; exit 1
			;;
	esac
	if [[ "${*:$OPTIND:1}" ]]; then
		output="${*:$OPTIND:1}"
	fi
}

prepare_build_id() {
	if [ -z "${KOKORO_BUILD_NUMBER}" ]; then
		echo eng-$(hostname)-$(date --utc)
	else
		echo ${KOKORO_BUILD_NUMBER}
	fi
}

install_prerequisites() {
	apt update
	packages=(
		apt-utils
		automake
		binfmt-support
		build-essential
		ca-certificates
		cmake
		curl
		debsums
		dosfstools
		fai-server
		fai-setup-storage
		fdisk
		git
		libjson-c-dev
		libtool
		libwebsockets-dev
		make
		protobuf-compiler
		python3
		python3-libcloud
		python3-marshmallow
		python3-pytest
		python3-yaml
		qemu-user-static
		qemu-utils
		sudo
		udev
	)
	if [[ "$arch" == "aarch64" ]]; then
		packages+=(
			gcc-aarch64-linux-gnu
			libc6-dev-arm64-cross
			qemu-system-arm
		)
	else
		packages+=(
			qemu-system
		)
	fi

	if [[ "$uboot" != 1 ]]; then
		packages+=(
			libguestfs-tools
			linux-image-generic
		)
	fi

	DEBIAN_FRONTEND=noninteractive \
	apt install --no-install-recommends --assume-yes "${packages[@]}"

	if [ ! -f $"HOME"/.cargo/bin/cargo ]; then
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
	fi

	source "$HOME"/.cargo/env
	rustup target add "${arch}"-unknown-linux-gnu
	cargo install cargo-license
	cargo install cargo-deb
}

download_debian_cloud_image() {
	local ver=38da93fe
	local prj=debian-cloud-images
	local url="https://salsa.debian.org/cloud-team/${prj}/-/archive/${ver}/${prj}-${ver}.tar.gz"
	local outdir="${debian_cloud_image}"

	mkdir -p "${outdir}"
	wget -O - "${url}" | tar xz -C "${outdir}" --strip-components=1
}

build_rust_as_deb() {
	pushd "$SCRIPT_DIR/../../guest/$1" > /dev/null
	cargo deb \
		--target "${arch}-unknown-linux-gnu" \
		--output "${debian_cloud_image}/localdebs"
	popd > /dev/null
}

build_ttyd() {
	local ttyd_version=1.7.7
	local url="https://github.com/tsl0922/ttyd/archive/refs/tags/${ttyd_version}.tar.gz"
	cp -r "$SCRIPT_DIR/ttyd" "${workdir}/ttyd"

	pushd "${workdir}" > /dev/null
	wget "${url}" -O - | tar xz
	cp ttyd/* ttyd-${ttyd_version}/scripts
	pushd "$workdir/ttyd-${ttyd_version}" > /dev/null
	bash -c "env BUILD_TARGET=${arch} ./scripts/cross-build.sh"
	mkdir -p "${dst}/files/usr/local/bin/ttyd"
	cp "/tmp/stage/${arch}-linux-musl/bin/ttyd" "${dst}/files/usr/local/bin/ttyd/AVF"
	chmod 777 "${dst}/files/usr/local/bin/ttyd/AVF"
	mkdir -p "${dst}/files/usr/share/doc/ttyd"
	cp LICENSE "${dst}/files/usr/share/doc/ttyd/copyright"
	popd > /dev/null
	popd > /dev/null
}

copy_android_config() {
	local src
	local dst
	src="$SCRIPT_DIR/fai_config"
	dst="${config_space}"

	cp -R "${src}"/* "${dst}"
	cp "$SCRIPT_DIR/image.yaml" "${resources_dir}"

	cp -R "$SCRIPT_DIR/localdebs/" "${debian_cloud_image}/"
	build_ttyd
	build_rust_as_deb forwarder_guest
	build_rust_as_deb forwarder_guest_launcher
	build_rust_as_deb shutdown_runner
	build_rust_as_deb storage_balloon_agent
}

package_custom_kernel() {
	if [[ "$use_generic_kernel" == 1 ]]; then
		# NOTE: For bpfcc-tools, install generic headers for the generic kernel.
		cat > "${config_space}/package_config/LAST" <<EOF
PACKAGES install
linux-headers-generic
EOF
		return
	fi

	# NOTE: Prevent FAI from installing a default Debian kernel, by removing
	#       linux-image meta package names from arch-specific class files.
	sed -i "/linux-image.*-${debian_arch}/d" \
	    "${config_space}/package_config/${debian_arch^^}"

	cmd_args=(
		-a "$arch"
		-d "${debian_cloud_image}/localdebs"
	)

	if [[ "$save_workdir" -eq 1 ]]; then
		cmd_args+=(-w)
	fi
	$SCRIPT_DIR/build_custom_kernel.sh "${cmd_args[@]}"
	# 4. Add the package to package_config/AVF
	abi_flavour=$(cat "${debian_cloud_image}/localdebs/abi_flavour")
	cat >> "${config_space}/package_config/AVF" <<EOF
linux-headers-${abi_flavour}
linux-image-${abi_flavour}-unsigned
EOF
}

run_fai() {
	# NOTE: Prevent FAI from installing grub packages and running related scripts,
	#       if we are loading the kernel directly.
	if [[ "$uboot" != 1 ]]; then
		sed -i "/shim-signed/d ; /grub.*${debian_arch}.*/d" \
		    "${config_space}/package_config/${debian_arch^^}"
		rm "${config_space}/scripts/SYSTEM_BOOT/20-grub"
	fi

	local out="${raw_disk_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
}

generate_output_package() {
	fdisk -l "${raw_disk_image}"
	local root_partition_num=1
	local efi_partition_num=15

	local vm_config="$SCRIPT_DIR/vm_config.json"
	if [[ "$uboot" == 1 ]]; then
		vm_config="$SCRIPT_DIR/vm_config.u-boot.json"
	fi

	pushd ${workdir} > /dev/null

	echo ${build_id} > build_id

	loop=$(losetup -f --show --partscan $raw_disk_image)
	dd if="${loop}p$root_partition_num" of=root_part
	dd if="${loop}p$efi_partition_num" of=efi_part
	losetup -d "${loop}"

	cp ${vm_config} vm_config.json
	# TODO(b/363985291): remove this when ballooning is supported on generic kernel
	if [[ "$use_generic_kernel" == 1 ]] && [[ "$arch" == "aarch64" ]]; then
		sed -i 's/"auto_memory_balloon": true/"auto_memory_balloon": false/g' vm_config.json
	fi

	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $raw_disk_image $root_partition_num)/g" vm_config.json
	sed -i "s/{efi_part_guid}/$(sfdisk --part-uuid $raw_disk_image $efi_partition_num)/g" vm_config.json

	contents=(
		build_id
		root_part
		efi_part
		vm_config.json
	)

	if [[ "$uboot" != 1 ]]; then
		rm -f vmlinuz* initrd.img*
		virt-get-kernel -a "${raw_disk_image}"
		mv vmlinuz* vmlinuz
		mv initrd.img* initrd.img

		if [[ "$arch" == "aarch64" ]]; then
			lz4 -BD -12 -q vmlinuz vmlinuz.lz4
			mv vmlinuz.lz4 vmlinuz
		fi

		contents+=(
			vmlinuz
			initrd.img
		)
	fi

	popd > /dev/null

	# --sparse option isn't supported in apache-commons-compress
	tar czv -f ${output} -C ${workdir} "${contents[@]}"
}

clean_up() {
	[ "$save_workdir" -eq 1 ] || rm -rf "${workdir}"
}

set -e
trap clean_up EXIT

output=images.tar.gz
workdir=$(mktemp -d)
raw_disk_image=${workdir}/image.raw
build_id=$(prepare_build_id)
debian_cloud_image=${workdir}/debian_cloud_image
debian_version=bookworm
config_space=${debian_cloud_image}/config_space/${debian_version}
resources_dir=${debian_cloud_image}/src/debian_cloud_images/resources
arch="$(uname -m)"
mode=debug
save_workdir=0
use_generic_kernel=0
uboot=0

parse_options "$@"
check_sudo
install_prerequisites
download_debian_cloud_image
copy_android_config
package_custom_kernel
run_fai
generate_output_package
