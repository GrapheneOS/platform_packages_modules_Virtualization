#!/bin/bash

# This is a script to build a Debian image that can run in a VM created via AVF.
# TODOs:
# - Add Android-specific packages via a new class
# - Use a stable release from debian-cloud-images

show_help() {
	echo "Usage: sudo $0 [OPTION]... [FILE]"
	echo "Builds a debian image and save it to FILE. [sudo is required]"
	echo "Options:"
	echo "-h         Print usage and this help message and exit."
	echo "-a ARCH    Architecture of the image [default is aarch64]"
	echo "-r         Release mode build"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root."
		exit
	fi
}

parse_options() {
	while getopts "hra:" option; do
		case ${option} in
			h)
				show_help
				exit;;
			a)
				if [[ "$OPTARG" != "aarch64" && "$OPTARG" != "x86_64" ]]; then
					echo "Invalid architecture: $OPTARG"
					exit
				fi
				arch="$OPTARG"
				if [[ "$arch" == "x86_64" ]]; then
					debian_arch="amd64"
				fi
				;;
			r)
				mode=release
				;;
			*)
				echo "Invalid option: $OPTARG"
				exit
				;;
		esac
	done
	if [[ "${*:$OPTIND:1}" ]]; then
		built_image="${*:$OPTIND:1}"
	fi
}

install_prerequisites() {
	apt update
	packages=(
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

	# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
	if [[ "$arch" == "x86_64" ]]; then
		packages+=(
			libguestfs-tools
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
}

download_debian_cloud_image() {
	local ver=master
	local prj=debian-cloud-images
	local url="https://salsa.debian.org/cloud-team/${prj}/-/archive/${ver}/${prj}-${ver}.tar.gz"
	local outdir="${debian_cloud_image}"

	mkdir -p "${outdir}"
	wget -O - "${url}" | tar xz -C "${outdir}" --strip-components=1
}

build_rust_binary_and_copy() {
	pushd "$(dirname "$0")/../../guest/$1" > /dev/null
	local release_flag=
	local artifact_mode=debug
	if [[ "$mode" == "release" ]]; then
		release_flag="--release"
		artifact_mode=release
	fi
	RUSTFLAGS="-C linker=${arch}-linux-gnu-gcc" cargo build \
		--target "${arch}-unknown-linux-gnu" \
		--target-dir "${workdir}/$1" ${release_flag}
	mkdir -p "${dst}/files/usr/local/bin/$1"
	cp "${workdir}/$1/${arch}-unknown-linux-gnu/${artifact_mode}/$1" "${dst}/files/usr/local/bin/$1/AVF"
	chmod 777 "${dst}/files/usr/local/bin/$1/AVF"

	mkdir -p "${dst}/files/usr/share/doc/$1"
	cargo license > "${dst}/files/usr/share/doc/$1/copyright"
	popd > /dev/null
}

build_ttyd() {
	local ttyd_version=1.7.7
	local url="https://github.com/tsl0922/ttyd/archive/refs/tags/${ttyd_version}.tar.gz"
	cp -r $(dirname $0)/ttyd ${workdir}/ttyd

	pushd "${workdir}" > /dev/null
	wget "${url}" -O - | tar xz
	cp ttyd/* ttyd-${ttyd_version}/scripts
	pushd "$workdir/ttyd-${ttyd_version}" > /dev/null
	bash -c "env BUILD_TARGET=${arch} ./scripts/cross-build.sh"
	mkdir -p "${dst}/files/usr/local/bin/ttyd"
	cp /tmp/stage/${arch}-linux-musl/bin/ttyd "${dst}/files/usr/local/bin/ttyd/AVF"
	chmod 777 "${dst}/files/usr/local/bin/ttyd/AVF"
	mkdir -p "${dst}/files/usr/share/doc/ttyd"
	cp LICENSE "${dst}/files/usr/share/doc/ttyd/copyright"
	popd > /dev/null
	popd > /dev/null
}

copy_android_config() {
	local src="$(dirname "$0")/fai_config"
	local dst="${config_space}"

	cp -R "${src}"/* "${dst}"
	cp "$(dirname "$0")/image.yaml" "${resources_dir}"

	build_ttyd
	build_rust_binary_and_copy forwarder_guest
	build_rust_binary_and_copy forwarder_guest_launcher
	build_rust_binary_and_copy ip_addr_reporter
}

run_fai() {
	local out="${built_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
}

extract_partitions() {
	root_partition_num=1
	efi_partition_num=15

	loop=$(losetup -f --show --partscan $built_image)
	dd if=${loop}p$root_partition_num of=root_part
	dd if=${loop}p$efi_partition_num of=efi_part
	losetup -d ${loop}

	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $built_image $root_partition_num)/g" vm_config.json
	sed -i "s/{efi_part_guid}/$(sfdisk --part-uuid $built_image $efi_partition_num)/g" vm_config.json
}

clean_up() {
	rm -rf "${workdir}"
}

set -e
trap clean_up EXIT

built_image=image.raw
workdir=$(mktemp -d)
debian_cloud_image=${workdir}/debian_cloud_image
debian_version=bookworm
config_space=${debian_cloud_image}/config_space/${debian_version}
resources_dir=${debian_cloud_image}/src/debian_cloud_images/resources
arch=aarch64
debian_arch=arm64
mode=debug
parse_options "$@"
check_sudo
install_prerequisites
download_debian_cloud_image
copy_android_config
run_fai
fdisk -l "${built_image}"
images=()

cp $(dirname $0)/vm_config.json.${arch} vm_config.json

if [[ "$arch" == "aarch64" ]]; then
	extract_partitions
	images+=(
		root_part
		efi_part
	)
fi

# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
if [[ "$arch" == "x86_64" ]]; then
	virt-get-kernel -a "${built_image}"
	mv vmlinuz* vmlinuz
	mv initrd.img* initrd.img
	images+=(
		"${built_image}"
		vmlinuz
		initrd.img
	)
fi

# --sparse option isn't supported in apache-commons-compress
tar czv -f images.tar.gz ${images[@]} vm_config.json
