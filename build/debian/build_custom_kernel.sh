#!/bin/bash

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

parse_options() {
	while getopts "a:d:w" option; do
		case ${option} in
			d)
				dest_dir="$OPTARG"
				;;
			a)
				arch="$OPTARG"
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

build_custom_kernel() {
	local deb_base_url="https://deb.debian.org/debian"
	local deb_security_base_url="https://security.debian.org/debian-security"

	local pool_dir="pool/main/l/linux"
	local ksrc_base_url="${deb_base_url}/${pool_dir}"
	local ksrc_security_base_url="${deb_security_base_url}/${pool_dir}"

	# NOTE: 6.1 is the latest LTS kernel for which Debian's kernel build scripts
	#       work on Python 3.10, the default version on our Ubuntu 22.04 builders.
	#
	#       We track the latest Debian stable kernel version for the 6.1 branch,
	#       which can be found at:
	#       https://packages.debian.org/stable/linux-source-6.1
	local debian_kver="6.1.123-1"

	local dsc_file="linux_${debian_kver}.dsc"
	local orig_ksrc_file="linux_${debian_kver%-*}.orig.tar.xz"
	local debian_ksrc_file="linux_${debian_kver}.debian.tar.xz"

	# 0. Grab the kernel sources, and the latest debian keyrings
	mkdir -p "${workdir}/kernel"
	pushd "${workdir}/kernel" > /dev/null

	wget "${ksrc_security_base_url}/${dsc_file}" || \
	wget "${ksrc_base_url}/${dsc_file}"

	wget "${ksrc_security_base_url}/${orig_ksrc_file}" || \
	wget "${ksrc_base_url}/${orig_ksrc_file}"

	wget "${ksrc_security_base_url}/${debian_ksrc_file}" || \
	wget "${ksrc_base_url}/${debian_ksrc_file}"

	rsync -az --progress keyring.debian.org::keyrings/keyrings/ /usr/share/keyrings/

	# 1. Verify, extract and merge patches into the original kernel sources
	dpkg-source --require-strong-checksums \
	            --require-valid-signature \
	            --extract "${dsc_file}"
	pushd "linux-${debian_kver%-*}" > /dev/null

	local kpatches_src="$SCRIPT_DIR/kernel/patches"
	cp -r "${kpatches_src}/avf" debian/patches/
	cat "${kpatches_src}/series" >> debian/patches/series
	./debian/rules orig

	local custom_flavour="avf"
	local debarch_flavour="${custom_flavour}-${debian_arch}"

	local abi_kver="$(sed -nE 's;Package: linux-support-(.*);\1;p' debian/control)"
	local abi_flavour="${abi_kver}-${debarch_flavour}"

	# 2. Define our custom flavour and regenerate control file
	# NOTE: Our flavour extends Debian's `cloud` config on the `none` featureset.
	cp "$SCRIPT_DIR/kernel/config" \
	   debian/config/${debian_arch}/config.${debarch_flavour}

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

	popd > /dev/null


	# 4. Copy the packages to the destination dir
	cp "linux-headers-${abi_flavour}_${debian_kver}_${debian_arch}.deb" \
	   "linux-image-${abi_flavour}-unsigned_${debian_kver}_${debian_arch}.deb" \
	   "$dest_dir"
	echo "${abi_flavour}" > "$dest_dir/abi_flavour"
}

install_prerequisites() {
	apt update
	packages+=(
		bc
		bison
		build-essential
		ca-certificates
		debhelper
		dh-exec
		flex
		gcc-12
		kernel-wedge
		libelf-dev
		libpci-dev
		libssl-dev
		lz4
		pahole
		python3-docutils
		python3-jinja2
		quilt
		rsync
		wget
	)
	if [[ "$arch" == "aarch64" ]]; then
		packages+=(
			gcc-12-aarch64-linux-gnu
			gcc-arm-linux-gnueabihf
			libc6-dev-arm64-cross
		)
	fi
	DEBIAN_FRONTEND=noninteractive \
	apt install --no-install-recommends --assume-yes "${packages[@]}"
}

clean_up() {
	[ "$save_workdir" -eq 1 ] || rm -rf "${workdir}"
}

set -e
trap clean_up EXIT

save_workdir=0
dest_dir=$SCRIPT_DIR
workdir=$(mktemp -d)
echo $workdir

parse_options "$@"
install_prerequisites
build_custom_kernel
