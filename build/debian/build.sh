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
	echo "-k         Build and use our custom kernel [default is cloud kernel]"
	echo "-r         Release mode build"
	echo "-w         Save temp work directory [for debugging]"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root." ; exit 1
	fi
}

parse_options() {
	while getopts "a:hkrw" option; do
		case ${option} in
			h)
				show_help ; exit
				;;
			a)
				arch="$OPTARG"
				;;
			k)
				use_custom_kernel=1
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

	# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
	if [[ "$arch" == "x86_64" ]]; then
		packages+=(
			libguestfs-tools
			linux-image-generic
		)
	fi

	if [[ "$use_custom_kernel" -eq 1 ]]; then
		packages+=(
			bc
			bison
			debhelper
			dh-exec
			flex
			gcc-12
			kernel-wedge
			libelf-dev
			libpci-dev
			lz4
			pahole
			python3-jinja2
			python3-docutils
			quilt
			rsync
		)
		if [[ "$arch" == "aarch64" ]]; then
			packages+=(
				gcc-arm-linux-gnueabihf
				gcc-12-aarch64-linux-gnu
			)
		fi
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
	build_rust_binary_and_copy shutdown_runner
}

package_custom_kernel() {
	if [[ "$use_custom_kernel" != 1 ]]; then
		echo "linux-headers-generic" >> "${config_space}/package_config/AVF"
		return
	fi

	# NOTE: 6.1 is the latest LTS kernel for which Debian's kernel build scripts
	#       work on Python 3.10, the default version on our Ubuntu 22.04 builders.
	local debian_kver="6.1.119-1"
	local custom_flavour="avf"
	local ksrc_base_url="https://deb.debian.org/debian/pool/main/l/linux"

	local dsc_url="${ksrc_base_url}/linux_${debian_kver}.dsc"
	local debian_ksrc_url="${ksrc_base_url}/linux_${debian_kver}.debian.tar.xz"
	local orig_ksrc_url="${ksrc_base_url}/linux_${debian_kver%-*}.orig.tar.xz"

	# 0. Grab the kernel sources, and the latest debian keyrings
	mkdir -p "${workdir}/kernel"
	pushd "${workdir}/kernel" > /dev/null
	wget "$dsc_url"
	wget "$orig_ksrc_url"
	wget "$debian_ksrc_url"
	rsync -az --progress keyring.debian.org::keyrings/keyrings/ /usr/share/keyrings/

	# 1. Verify, extract and merge patches into the original kernel sources
	dpkg-source --require-strong-checksums \
	            --require-valid-signature \
	            --extract linux_${debian_kver}.dsc
	pushd "linux-${debian_kver%-*}" > /dev/null
	# TODO: Copy our own kernel patches to debian/patches
	#       and add patch file names in the desired order to debian/patches/series
	./debian/rules orig

	local abi_kver="$(sed -nE 's;Package: linux-support-(.*);\1;p' debian/control)"
	local debarch_flavour="${custom_flavour}-${debian_arch}"
	local abi_flavour="${abi_kver}-${debarch_flavour}"

	# 2. Define our custom flavour and regenerate control file
	# NOTE: Our flavour extends Debian's `cloud` config on the `none` featureset.
	cat > debian/config/${debian_arch}/config.${debarch_flavour} <<EOF
# TODO: Add our custom kernel config to this file
EOF

	sed -z "s;\[base\]\nflavours:;[base]\nflavours:\n ${debarch_flavour};" \
	    -i debian/config/${debian_arch}/none/defines
	cat >> debian/config/${debian_arch}/none/defines <<EOF
[${debarch_flavour}_image]
configs:
 config.cloud
 ${debian_arch}/config.${debarch_flavour}
EOF
	cat >> debian/config/${debian_arch}/defines <<EOF
[${debarch_flavour}_description]
hardware: ${arch} AVF
hardware-long: ${arch} Android Virtualization Framework
EOF
	./debian/rules debian/control || true

	# 3. Build the kernel and generate Debian packages
	./debian/rules source
	[[ "$arch" == "$(uname -m)" ]] || export $(dpkg-architecture -a $debian_arch)
	make -j$(nproc) -f debian/rules.gen \
	     "binary-arch_${debian_arch}_none_${debarch_flavour}"

	# 4. Copy the packages to localdebs and add their names to package_config/AVF
	popd > /dev/null
	cp "linux-headers-${abi_flavour}_${debian_kver}_${debian_arch}.deb" \
	   "linux-image-${abi_flavour}-unsigned_${debian_kver}_${debian_arch}.deb" \
	   "${debian_cloud_image}/localdebs/"
	popd > /dev/null
	cat >> "${config_space}/package_config/AVF" <<EOF
linux-headers-${abi_flavour}
linux-image-${abi_flavour}-unsigned
EOF
}

run_fai() {
	local out="${raw_disk_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
}

generate_output_package() {
	fdisk -l "${raw_disk_image}"
	root_partition_num=1
	bios_partition_num=14
	efi_partition_num=15

	pushd ${workdir} > /dev/null

	echo ${build_id} > build_id

	loop=$(losetup -f --show --partscan $raw_disk_image)
	dd if="${loop}p$root_partition_num" of=root_part
	if [[ "$arch" == "x86_64" ]]; then
		dd if="${loop}p$bios_partition_num" of=bios_part
	fi
	dd if="${loop}p$efi_partition_num" of=efi_part
	losetup -d "${loop}"

	cp ${vm_config} vm_config.json
	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $raw_disk_image $root_partition_num)/g" vm_config.json
	if [[ "$arch" == "x86_64" ]]; then
		sed -i "s/{bios_part_guid}/$(sfdisk --part-uuid $raw_disk_image $bios_partition_num)/g" vm_config.json
	fi
	sed -i "s/{efi_part_guid}/$(sfdisk --part-uuid $raw_disk_image $efi_partition_num)/g" vm_config.json

	images=()
	if [[ "$arch" == "aarch64" ]]; then
		images+=(
			root_part
			efi_part
		)
	# TODO(b/365955006): remove these lines when uboot supports x86_64 EFI application
	elif [[ "$arch" == "x86_64" ]]; then
		rm -f vmlinuz initrd.img
		virt-get-kernel -a "${raw_disk_image}"
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

	popd > /dev/null

	# --sparse option isn't supported in apache-commons-compress
	tar czv -f ${output} -C ${workdir} build_id "${images[@]}" vm_config.json
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
vm_config="$(realpath $(dirname "$0"))/vm_config.json.${arch}"
mode=debug
save_workdir=0
use_custom_kernel=0

parse_options "$@"
check_sudo
install_prerequisites
download_debian_cloud_image
copy_android_config
package_custom_kernel
run_fai
generate_output_package
