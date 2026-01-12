#!/bin/bash

# Temporal manual build script for arm64 until this is re-wrriten in soong.bp
# This is to share reprodeable way to package cidata.iso
# TODO: Create a soong module to generate cidata.iso in dist.

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
TOP="${SCRIPT_DIR}/../../../../../"

pushd $TOP

DST_DIR=out/host/linux_musl-arm64
DST_BIN=${DST_DIR}/bin
DST_LIB=${DST_DIR}/lib64
XORRISO=out/host/linux-x86/bin/xorriso

rm -rf ${DST_DIR}
build/soong/soong_ui.bash --make-mode TARGET_PRODUCT=cf_arm64_only_phone TARGET_RELEASE=trunk_staging TARGET_BUILD_VARIANT=userdebug \
	${DST_BIN}/linux_vm_manager ${DST_BIN}/forwarder_guest ${XORRISO}

TMP=$(mktemp -d)

cp -R packages/modules/Virtualization/build/debian/cloud-init_config/* ${TMP}
mkdir -p ${TMP}/root_files/usr/bin/
mkdir -p ${TMP}/root_files/usr/lib64/

cp ${DST_BIN}/linux_vm_manager ${TMP}/root_files/usr/bin/
cp ${DST_BIN}/forwarder_guest ${TMP}/root_files/usr/bin/
cp ${DST_LIB}/* ${TMP}/root_files/usr/lib64/


CONFIG_HASH=$(find "${TMP}" -type f -exec sha256sum {} + | sort | sha256sum | cut -d' ' -f1 | cut -c1-16)
sed -i "s/{INSTANCE_ID}/${CONFIG_HASH}/g" "${TMP}/meta-data"

chmod -R o=g ${TMP}

${XORRISO} -as mkisofs -V cidata -J -uid 0 -gid 0 -o cidata.iso -R ${TMP}

popd

echo "Done. check cidata.iso for result, and ${TMP} for intermediates"
