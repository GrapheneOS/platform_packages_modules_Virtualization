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
	echo "-a ARCH       Architecture of the image [default is host arch: $(uname -m)]"
	echo "-b BUILD_ID   Set build id of the debian image [default is eng-1000000-\$(date --utc +'%a %b %d %H:%M:%S %Z %Y')]"
	echo "-k KERNEL_ID  Build ID for kernel [default is the last known good build]"
	echo "-h            Print usage and this help message and exit."
	echo "-w            Save temp work directory [for debugging]"
	echo "-W WORK_DIR   Specify work dir instead of temporarily creating. Imply -w [for debugging]"
}

check_sudo() {
	if [ "$EUID" -ne 0 ]; then
		echo "Please run as root." ; exit 1
	fi
}

parse_options() {
	while getopts "a:b:k:hwW:" option; do
		case ${option} in
			a)
				arch="$OPTARG"
				;;
			b)
				build_id="$OPTARG"
				;;
			k)
                kernel_build_id="$OPTARG"
                ;;
			h)
				show_help ; exit
				;;
			w)
				save_workdir=1
				;;
			W)
				workdir="${OPTARG%/}"
				save_workdir=1
				may_skip_build=1
				;;
			*)
				echo "Invalid option: $OPTARG" ; exit 1
				;;
		esac
	done
	case "$arch" in
		aarch64)
			debian_arch="arm64"
			vmlinuz_name="Image"
			;;
		x86_64)
			debian_arch="amd64"
			vmlinuz_name="bzImage"
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
	apt install software-properties-common -y
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
		fdisk
		genisoimage
		git
		jq
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
		wget
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

	if [ ! -f "$HOME"/.cargo/bin/cargo ]; then
		git clone https://github.com/rust-lang/rustup.git ${workdir}/rustup
		${workdir}/rustup/rustup-init.sh -y
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

	local img=debian-13-genericcloud-${debian_arch}.tar.xz
	local url="https://cloud.debian.org/images/cloud/${debian_version}/latest/${img}"
	wget -O - "${url}" | tar xJ -C "${outdir}"
}

build_rust_as_deb() {
	local dst="${cidata}/localdebs"

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
	local install_path="${chroot_ttyd}/usr/local/bin/ttyd"

	if [[ "$may_skip_build" == 1 && -f "${install_path}" ]]; then
		echo "Skipping build_ttyd(). ${install_path} already exists"
		return
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

	mkdir -p "${chroot_ttyd}/usr/local/bin" || true
	cp "${out}/bin/ttyd" "${chroot_ttyd}/usr/local/bin/ttyd"
	chmod 755 "${chroot_ttyd}/usr/local/bin/ttyd"
	mkdir -p "${chroot_ttyd}/usr/share/doc/ttyd/copyright"
	cp LICENSE "${chroot_ttyd}/usr/share/doc/ttyd/copyright/"
	popd > /dev/null
	popd > /dev/null
}

copy_android_config() {
	mkdir -p "${cidata}/localdebs" || true
	cp -avpR "$SCRIPT_DIR/cloud-init_config"/* "${cidata}"
	cp -avpR "$SCRIPT_DIR/localdebs/"* "${cidata}/localdebs" || true

	build_ttyd
	build_rust_as_deb forwarder_guest
	build_rust_as_deb forwarder_guest_launcher
	build_rust_as_deb shutdown_runner
	build_rust_as_deb storage_balloon_agent
}

build_cidata() {
	local dst="${workdir}/${cidata_image}"

	# repo doesn't fully keep ownership nor permission, so explicitly set here.
	chmod -R o=g "${cidata}"
	chown -R 0:0 "${cidata}"

	# Build CIDATA with ISO9660.
	# Need to clean first. otherwise try to append here.
	rm -rf "${dst}" || true
	genisoimage -output "${dst}" -V cidata -J -R "${cidata}"
}

generate_output_package() {
	local vm_config="$SCRIPT_DIR/vm_config.json"

	pushd ${workdir} > /dev/null

	echo ${build_id} > build_id

	local root_partition_num=1
	if [[ "$may_skip_build" == 0 || ! -f "root_part" ]]; then
		loop=$(losetup -f --show --partscan $raw_disk_image)
		dd if="${loop}p$root_partition_num" of=root_part
		losetup -d "${loop}"

		${SCRIPT_DIR}/chroot_rootfs.sh \
			-b "${SCRIPT_DIR}:/mnt/build" \
			-b "${chroot_ttyd}:/mnt/ttyd" \
			-c /mnt/build/build_rootfs_in_chroot.sh \
			root_part
	fi

	cp ${vm_config} vm_config.json

	sed -i "s/{root_part_guid}/$(sfdisk --part-uuid $raw_disk_image $root_partition_num)/g" vm_config.json

	if [[ -z "${kernel_build_id}" ]]; then
		kernel_build_id=$(curl https://ci.android.com/builds/branches/aosp_kernel-common-android16-6.12/status.json | \
			jq -r '.targets[] | select(.name == "kernel_server_'${arch}'") | .last_known_good_build')

		if [[ -z "${kernel_build_id}" || "${kernel_build_id}" == "null" ]]; then
			echo "ERROR: Failed to fetch the latest kernel build ID for ${arch}." >&2
			echo "The CI endpoint may be down or the build is missing." >&2
			echo "Please try specifying a build ID manually using the -k option." >&2
			exit 1
		fi
	fi

	wget -O vmlinuz https://androidbuildinternal.googleapis.com/android/internal/build/v3/builds/${kernel_build_id}/kernel_server_${arch}/attempts/latest/artifacts/${vmlinuz_name}/url
	wget -O initrd.img https://androidbuildinternal.googleapis.com/android/internal/build/v3/builds/${kernel_build_id}/kernel_server_${arch}/attempts/latest/artifacts/initramfs.img/url

	contents=(
		build_id
		root_part
		vm_config.json
		vmlinuz
		initrd.img
		${cidata_image}
	)

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
build_id=$(echo eng-1000000-$(date --utc +'%a %b %d %H:%M:%S %Z %Y'))
kernel_build_id=
debian_version=trixie
arch="$(uname -m)"
save_workdir=0
may_skip_build=0

parse_options "$@"

if [[ -n "${workdir}" ]]; then
	mkdir -p "${workdir}" || true
else
	workdir=$(mktemp -d)
fi

debian_cloud_image=${workdir}/debian_cloud_image
cidata=${debian_cloud_image}/cidata
cidata_image="cidata.iso"
chroot_ttyd=${debian_cloud_image}/chroot_ttyd
raw_disk_image=${debian_cloud_image}/disk.raw

install_prerequisites
download_debian_cloud_image
copy_android_config
build_cidata
generate_output_package
