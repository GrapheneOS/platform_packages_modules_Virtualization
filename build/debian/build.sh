#!/bin/bash

# This is a script to build a Debian image that can run in a VM created via AVF.
# TODOs:
# - Add Android-specific packages via a new class
# - Use a stable release from debian-cloud-images

set -x

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

show_help() {
	echo "Usage: sudo $0 [OPTION]... [FILE]"
	echo "Builds a debian image and save it to FILE. [sudo is required]"
	echo "Options:"
	echo "-a ARCH      Architecture of the image [default is host arch: $(uname -m)]"
	echo "-b BUILD_ID  Set build id [default is eng-\$(hostname)-\$(date --utc)]"
	echo "-g           Use Debian generic kernel [default is our custom kernel]"
	echo "-h           Print usage and this help message and exit."
	echo "-u           Set VM boot mode to u-boot [default is to load kernel directly]"
	echo "-w           Save temp work directory [for debugging]"
	echo "-W WORK_DIR  Specify work dir instead of temporarily creating. Imply -w [for debugging]"
	echo "-c           Build with cloud-init"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root." ; exit 1
	fi
}

parse_options() {
	while getopts "a:b:ghuwW:c" option; do
		case ${option} in
			a)
				arch="$OPTARG"
				;;
			b)
				build_id="$OPTARG"
				;;
			g)
				use_generic_kernel=1
				;;
			h)
				show_help ; exit
				;;
			u)
				uboot=1
				;;
			w)
				save_workdir=1
				;;
			W)
				workdir="${OPTARG%/}"
				save_workdir=1
				may_skip_build=1
				;;
			c)
				cloud_init=1
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

	if [[ "$cloud_init" == 1 ]]; then
		packages+=(
			genisoimage
		)
	fi

	DEBIAN_FRONTEND=noninteractive \
		apt install --no-install-recommends --assume-yes "${packages[@]}"

	if [ ! -f $"HOME"/.cargo/bin/cargo ]; then
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
	fi

	source "$HOME"/.cargo/env
	rustup target add "${arch}"-unknown-linux-gnu
  cargo install cargo-license --version 0.6.1
  cargo install cargo-deb --version 3.3.0
}

download_debian_cloud_image() {
	if [[ "$may_skip_build" == 1 && -f "${raw_disk_image}" ]]; then
		echo "Skipping download_debian_cloud_image(). ${raw_disk_image} already exists"
		return
	fi

	local outdir="${debian_cloud_image}"
	mkdir -p "${outdir}" || true

	if [[ "$cloud_init" == 1 ]]; then
		local img=debian-12-genericcloud-${debian_arch}.tar.xz
		local url="https://cloud.debian.org/images/cloud/${debian_version}/latest/${img}"
		wget -O - "${url}" | tar xJ -C "${outdir}"
	else
		local ver=38da93fe
		local prj=debian-cloud-images
		local url="https://salsa.debian.org/cloud-team/${prj}/-/archive/${ver}/${prj}-${ver}.tar.gz"

		wget -O - "${url}" | tar xz -C "${outdir}" --strip-components=1
	fi
}

build_rust_as_deb() {
	if [[ "$cloud_init" == 1 ]]; then
		local dst="${cidata}/localdebs"
	else
		local dst="${debian_cloud_image}/localdebs"
	fi

	# deb file format: ${name}_${version}_${arch}.deb)
	local name="${1//_/-}"
	local old=$(find ${dst} -maxdepth 1 -name "${name}_*.deb")
	if [[ "$may_skip_build" == 1 && -n "${old}" ]]; then
		echo "Skipping build_rust_as_deb(${1}). ${old} already exists"
		return
	fi

	pushd "$SCRIPT_DIR/../../guest/$1" > /dev/null
	cargo deb \
		--target "${arch}-unknown-linux-gnu" \
		--output "${dst}"
	popd > /dev/null
}

build_ttyd() {
	if [[ "$cloud_init" == 1 ]]; then
		local install_path="${cidata}/files/usr/local/bin/ttyd"

		if [[ "$may_skip_build" == 1 && -f "${install_path}" ]]; then
			echo "Skipping build_ttyd(). ${install_path} already exists"
			return
		fi
	else
		local dst="${config_space}"
		local install_path="${dst}/files/usr/local/bin/ttyd"

		if [[ "$may_skip_build" == 1 && -d "${install_path}" ]]; then
			echo "Skipping build_ttyd(). ${install_path} already exists"
			return
		fi
	fi

	local ttyd_version=1.7.7
	local url="https://github.com/tsl0922/ttyd/archive/refs/tags/${ttyd_version}.tar.gz"
	local build_env=(
		"BUILD_TARGET=${arch}"
		"CROSS_ROOT=${workdir}/tmp.ttyd/cross"
		"STAGE_ROOT=${workdir}/tmp.ttyd/stage"
		"BUILD_ROOT=${workdir}/tmp.ttyd/build"
	)
	local out="${workdir}/tmp.ttyd/stage/${arch}-linux-musl"

	cp -r "$SCRIPT_DIR/ttyd/" "${workdir}"

	pushd "${workdir}" > /dev/null
	wget "${url}" -O - | tar xz
	cp ttyd/* ttyd-${ttyd_version}/scripts
	pushd "$workdir/ttyd-${ttyd_version}" > /dev/null
	bash -c "env ${build_env[*]} ./scripts/cross-build.sh"

	if [[ "$cloud_init" == 1 ]]; then
		mkdir -p "${cidata}/files/usr/local/bin" || true
		cp "${out}/bin/ttyd" "${cidata}/files/usr/local/bin/ttyd"
		chmod 777 "${cidata}/files/usr/local/bin/ttyd"
		mkdir -p "${cidata}/files/usr/share/doc/ttyd/copyright"
		cp LICENSE "${cidata}/files/usr/share/doc/ttyd/copyright/"
	else
		mkdir -p "${dst}/files/usr/local/bin/ttyd"
		cp "${out}/bin/ttyd" "${dst}/files/usr/local/bin/ttyd/AVF"
		chmod 777 "${dst}/files/usr/local/bin/ttyd/AVF"
		mkdir -p "${dst}/files/usr/share/doc/ttyd/copyright/LICENSE"
		cp LICENSE "${dst}/files/usr/share/doc/ttyd/copyright/LICENSE/AVF"
	fi
	popd > /dev/null
	popd > /dev/null
}

copy_android_config() {
	if [[ "$cloud_init" == 1 ]]; then
		mkdir -p "${cidata}/localdebs" || true
		cp -vpR "$SCRIPT_DIR/cloud-init_config"/* "${cidata}"
		cp -vpR "$SCRIPT_DIR/localdebs/"* "${cidata}/localdebs" || true
	else
		local src="$SCRIPT_DIR/fai_config"
		local dst="${config_space}"

		cp -R "${src}"/* "${dst}"
		cp "$SCRIPT_DIR/image.yaml" "${resources_dir}"

		cp -R "$SCRIPT_DIR/localdebs/" "${debian_cloud_image}/"
	fi

	build_ttyd
	build_rust_as_deb forwarder_guest
	build_rust_as_deb forwarder_guest_launcher
	build_rust_as_deb shutdown_runner
	build_rust_as_deb storage_balloon_agent
}

package_custom_kernel() {
	if [[ "$cloud_init" != 1 ]]; then
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
	fi

	cmd_args=(
		-a "$arch"
		-d "$workdir"
	)

	if [[ "$save_workdir" -eq 1 ]]; then
		cmd_args+=(-W "${workdir}/tmp.kernel")
	fi
	$SCRIPT_DIR/build_custom_kernel.sh "${cmd_args[@]}"
}

build_cidata() {
	local dst="${workdir}/${cidata_image}"

	# Build CIDATA with ISO9660.
	# Need to clean first. otherwise try to append here.
	rm -rf "${dst}"
	genisoimage -output "${dst}" -V cidata -J -R "${cidata}"
}

run_fai() {
	if [[ "$cloud_init" == 1 ]]; then
		echo "FAI shouldn't be configured for cloud-init" >&2
		exit 1
	fi

	# NOTE: Prevent FAI from installing grub packages and running related scripts,
	#       if we are loading the kernel directly.
	if [[ "$uboot" != 1 ]]; then
		sed -i "/\/boot\/efi/d" \
			"${config_space}/files/etc/fstab/${debian_arch^^}"
		sed -i "/shim-signed/d ; /grub.*${debian_arch}.*/d" \
			"${config_space}/package_config/${debian_arch^^}"
		rm "${config_space}/scripts/SYSTEM_BOOT/20-grub" || true
	fi

	local out="${raw_disk_image}"
	make -C "${debian_cloud_image}" "image_bookworm_nocloud_${debian_arch}"
	mv "${debian_cloud_image}/image_bookworm_nocloud_${debian_arch}.raw" "${out}"
}

build_debian() {
	if [[ "$cloud_init" == 1 ]]; then
		build_cidata
	else
		run_fai
	fi
}

generate_output_package() {
	local vm_config="$SCRIPT_DIR/vm_config.json"
	if [[ "$cloud_init" == 1 ]]; then
		vm_config="$SCRIPT_DIR/vm_config.cloud-init.json"
	elif [[ "$uboot" == 1 ]]; then
		vm_config="$SCRIPT_DIR/vm_config.u-boot.json"
	fi

	pushd ${workdir} > /dev/null

	echo ${build_id} > build_id

	local root_partition_num=1
	loop=$(losetup -f --show --partscan $raw_disk_image)
	dd if="${loop}p$root_partition_num" of=root_part
	losetup -d "${loop}"

	cp ${vm_config} vm_config.json
	# TODO(b/363985291): remove this when ballooning is supported on generic kernel
	if [[ "$use_generic_kernel" == 1 ]] && [[ "$arch" == "aarch64" ]]; then
		sed -i 's/"auto_memory_balloon": true/"auto_memory_balloon": false/g' vm_config.json
	fi

	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $raw_disk_image $root_partition_num)/g" vm_config.json

	contents=(
		build_id
		root_part
		vm_config.json
	)
	if [[ "$uboot" == 0 ]]; then
		contents+=(
			vmlinuz
			initrd.img
			kernel_extras_part
		)
	fi
	if [[ "$uboot" == 1 || "$cloud_init" == 1 ]]; then
		local efi_partition_num=15
		local guid="$(sfdisk --part-uuid $raw_disk_image $efi_partition_num)"

		if [[ "$uboot" == 1 ]]; then
			loop=$(losetup -f --show --partscan $raw_disk_image)
			dd if="${loop}p$efi_partition_num" of=efi_part
			losetup -d "${loop}"
		else
			# For cloud-init, EFI is only required by /etc/fstab.
			# Placeholder partition is enough.
			# TODO: Modify /etc/fstab when we preprocess rootfs here as well.
			dd if=/dev/zero of=efi_part bs=1M count=1
			mkfs.fat -F 32 -c efi_part
		fi

		sed -i "s/{efi_part_guid}/${guid}/g" vm_config.json

		contents+=(
			efi_part
		)
	fi

	if [[ "$cloud_init" == 1 ]]; then
		contents+=(
			${cidata_image}
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

check_sudo

output=images.tar.gz
build_id=$(echo eng-$(hostname)-$(date --utc))
debian_version=bookworm
arch="$(uname -m)"
save_workdir=0
may_skip_build=0
use_generic_kernel=0
uboot=0
cloud_init=0

parse_options "$@"

if [[ -n "${workdir}" ]]; then
	mkdir -p "${workdir}" || true
else
	workdir=$(mktemp -d)
fi

debian_cloud_image=${workdir}/debian_cloud_image
if [[ "$cloud_init" == 1 ]]; then
	cidata=${debian_cloud_image}/cidata
	raw_disk_image=${debian_cloud_image}/disk.raw
	cidata_image="cidata.iso"
else
	raw_disk_image=${workdir}/image.raw
	config_space=${debian_cloud_image}/config_space/${debian_version}
	resources_dir=${debian_cloud_image}/src/debian_cloud_images/resources
fi

install_prerequisites
download_debian_cloud_image
copy_android_config
package_custom_kernel
build_debian
generate_output_package
