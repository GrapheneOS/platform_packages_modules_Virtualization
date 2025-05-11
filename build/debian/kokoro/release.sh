#!/bin/bash

# This is a script to release the Debian image built by Kokoro to Lorry.

set -e

show_help() {
	echo "Usage: $0 [OPTION]..."
	echo "Fetches a debian image from Placer and releases it to /android/ferrochrome/ARCH/TAG"
	echo "Options:"
	echo "-h            Print usage and this help message and exit."
	echo "-a ARCH       Architecture of the image. Defaults to all supported architectures."
	echo "-b BUILD_ID   Build ID to fetch. If omitted, latest build ID is selected."
	echo "-t TAG        Tag name to attach to the release. Defaults to BUILD_ID."
	echo "-c CONFIG     Kokoro config name. Default to continuous."
}

parse_opt() {
	while getopts "ha:b:t:c:" option; do
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
				;;
			b)
				build_id="$OPTARG"
				;;
			t)
				tag="$OPTARG"
				;;
			c)
				config="$OPTARG"
				;;
			*)
				echo "Invalid option: $OPTARG"
				exit
				;;
		esac
	done

	if [ "${build_id}" != "latest" ]; then
		echo "Build ID is ambiguous when architecture is not set"
		exit
	fi
}

arch=all
build_id=latest
tag=
placer_url="/placer/prod/home/kokoro-dedicated/build_artifacts/prod/ferrochrome"
image_filename="images.tar.gz"
config="continuous"

get_build_id() {
	local arch=$1
	local build_id=$2
	if [ "${build_id}" == "latest" ]; then
		local pattern=${placer_url}/${arch}/${config}
		build_id=$(basename $(fileutil ls ${pattern} | sort -V | tail -1))
	fi
	if [ -z "${build_id}" ]; then
    exit 1
	fi
	echo ${build_id}
}

get_image_path() {
	local arch=$1
	local build_id=$2
	local pattern=${placer_url}/${arch}/${config}/${build_id}/*/${image_filename}
	image=$(fileutil ls ${pattern} | tail -1)
	if [ $? -ne 0 ]; then
		echo "Cannot find image"
		exit
	fi
	echo ${image}
}

do_release() {
	local arch=$1
	local build_id=$2

	build_id=$(get_build_id ${arch} ${build_id})
	echo "Using build ID ${build_id} for ${arch}"
	local image=$(get_image_path ${arch} ${build_id})

	local tag=${tag:-${build_id}}
	local serving_url=/android/ferrochrome/${tag}/${arch}/${image_filename}
	echo "Releasing ${image} to ${serving_url}"

	local request='payload : { url_path: '"\"${serving_url}\""' source_path : '"\"${image}\""' }'
	local id=$(stubby call blade:download-lorry-api LorryService.CreatePayloads "${request}" | cut -d\  -f2)
	echo "Done. Visit https://lorry.corp.google.com/view/${id} to get an approval for the release."
}

parse_opt "$@"

if [ "${arch}" == "all" ]; then
	do_release aarch64 ${build_id}
	do_release x86_64 ${build_id}
else
	do_release ${arch} ${build_id}
fi
