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
	echo "-a ARCH    Architecture of the image [default is host arch: $(uname -m)]"
	echo "-r         Release mode build"
	echo "-w         Save temp work directory [for debugging]"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root." ; exit 1
	fi
}

parse_options() {
	while getopts "a:hrw" option; do
		case ${option} in
			h)
				show_help ; exit
				;;
			a)
				arch="$OPTARG"
				;;
			r)
				mode=release
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
		built_image="${*:$OPTIND:1}"
	fi
}

prepare_build_id() {
	local filename=build_id
	if [ -z "${KOKORO_BUILD_NUMBER}" ]; then
		echo eng-$(hostname)-$(date --utc) > ${filename}
	else
		echo ${KOKORO_BUILD_NUMBER} > ${filename}
	fi
	echo ${filename}
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

	# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
	if [[ "$arch" == "x86_64" ]]; then
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
	cp -r "$(dirname "$0")/ttyd" "${workdir}/ttyd"

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
	src="$(dirname "$0")/fai_config"
	dst="${config_space}"

	cp -R "${src}"/* "${dst}"
	cp "$(dirname "$0")/image.yaml" "${resources_dir}"

	cp -R "$(dirname "$0")/localdebs/" "${debian_cloud_image}/"
	build_ttyd
	build_rust_binary_and_copy forwarder_guest
	build_rust_binary_and_copy forwarder_guest_launcher
	build_rust_binary_and_copy ip_addr_reporter
	build_rust_binary_and_copy shutdown_runner
}

run_fai() {
	local out="${built_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
}

extract_partitions() {
	root_partition_num=1
	bios_partition_num=14
	efi_partition_num=15

	loop=$(losetup -f --show --partscan $built_image)
	dd if="${loop}p$root_partition_num" of=root_part
	if [[ "$arch" == "x86_64" ]]; then
		dd if="${loop}p$bios_partition_num" of=bios_part
	fi
	dd if="${loop}p$efi_partition_num" of=efi_part
	losetup -d "${loop}"

	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $built_image $root_partition_num)/g" vm_config.json
	if [[ "$arch" == "x86_64" ]]; then
		sed -i "s/{bios_part_guid}/$(sfdisk --part-uuid $built_image $bios_partition_num)/g" vm_config.json
	fi
	sed -i "s/{efi_part_guid}/$(sfdisk --part-uuid $built_image $efi_partition_num)/g" vm_config.json
}

clean_up() {
	[ "$save_workdir" -eq 1 ] || rm -rf "${workdir}"
}

set -e
trap clean_up EXIT

built_image=image.raw
workdir=$(mktemp -d)
build_id=$(prepare_build_id)
debian_cloud_image=${workdir}/debian_cloud_image
debian_version=bookworm
config_space=${debian_cloud_image}/config_space/${debian_version}
resources_dir=${debian_cloud_image}/src/debian_cloud_images/resources
arch="$(uname -m)"
mode=debug
save_workdir=0

parse_options "$@"
check_sudo
install_prerequisites
download_debian_cloud_image
copy_android_config
run_fai
fdisk -l "${built_image}"
images=()

cp "$(dirname "$0")/vm_config.json.${arch}" vm_config.json

extract_partitions

if [[ "$arch" == "aarch64" ]]; then
	images+=(
		root_part
		efi_part
	)
# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
elif [[ "$arch" == "x86_64" ]]; then
	rm -f vmlinuz initrd.img
	virt-get-kernel -a "${built_image}"
	mv vmlinuz* vmlinuz
	mv initrd.img* initrd.img
	images+=(
		bios_part
		root_part
		efi_part
		vmlinuz
		initrd.img
	)
fi

# --sparse option isn't supported in apache-commons-compress
tar czv -f images.tar.gz ${build_id} "${images[@]}" vm_config.json
