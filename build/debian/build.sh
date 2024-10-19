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
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root."
		exit
	fi
}

parse_options() {
	while getopts "ha:" option; do
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
		binfmt-support
		build-essential
		ca-certificates
		curl
		debsums
		dosfstools
		fai-server
		fai-setup-storage
		fdisk
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
	DEBIAN_FRONTEND=noninteractive \
	apt install --no-install-recommends --assume-yes "${packages[@]}"

	if [ ! -f $"HOME"/.cargo/bin/cargo ]; then
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
	fi

	source "$HOME"/.cargo/env
	rustup target add "${arch}"-unknown-linux-gnu
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
	RUSTFLAGS="-C linker=${arch}-linux-gnu-gcc" cargo build \
		--target "${arch}-unknown-linux-gnu" \
		--target-dir "${workdir}/$1"
	mkdir -p "${dst}/files/usr/local/bin/$1"
	cp "${workdir}/$1/${arch}-unknown-linux-gnu/debug/$1" "${dst}/files/usr/local/bin/$1/AVF"
	chmod 777 "${dst}/files/usr/local/bin/$1/AVF"
	popd > /dev/null
}

copy_android_config() {
	local src="$(dirname "$0")/fai_config"
	local dst="${config_space}"

	cp -R "${src}"/* "${dst}"
	cp "$(dirname "$0")/image.yaml" "${resources_dir}"

	local ttyd_version=1.7.7
	local url="https://github.com/tsl0922/ttyd/releases/download/${ttyd_version}/ttyd.${arch}"
	mkdir -p "${dst}/files/usr/local/bin/ttyd"
	wget "${url}" -O "${dst}/files/usr/local/bin/ttyd/AVF"
	chmod 777 "${dst}/files/usr/local/bin/ttyd/AVF"

	build_rust_binary_and_copy forwarder_guest
	build_rust_binary_and_copy forwarder_guest_launcher
	build_rust_binary_and_copy ip_addr_reporter
}

run_fai() {
	local out="${built_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
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
parse_options "$@"
check_sudo
install_prerequisites
download_debian_cloud_image
copy_android_config
run_fai
fdisk -l image.raw
