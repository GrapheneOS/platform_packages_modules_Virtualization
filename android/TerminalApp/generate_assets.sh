#!/bin/bash
set -e

if [ "$#" -ne 1 ]; then
    echo "$0 <image.raw path>"
    echo "image.raw can be built with packages/modules/Virtualization/build/debian/build.sh"
    exit 1
fi
image_raw_path=$(realpath $1)
pushd $(dirname $0) > /dev/null
tempdir=$(mktemp -d)
asset_dir=./assets/linux
mkdir -p ${asset_dir}
echo Copy files...
pushd ${tempdir} > /dev/null
cp "${image_raw_path}" ${tempdir}
tar czvS -f images.tar.gz $(basename ${image_raw_path})
popd > /dev/null
cp vm_config.json ${asset_dir}
mv ${tempdir}/images.tar.gz ${asset_dir}
echo Calculating hash...
hash=$(cat ${asset_dir}/images.tar.gz ${asset_dir}/vm_config.json | sha1sum | cut -d' ' -f 1)
echo ${hash} > ${asset_dir}/hash
popd > /dev/null
echo Cleaning up...
rm -rf ${tempdir}

