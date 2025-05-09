#!/bin/bash

set -e

echo "Build info"
echo "  KOKORO_JOB_NAME = ${KOKORO_JOB_NAME}"
echo "  KOKORO_BUILD_ID = ${KOKORO_BUILD_ID}"
echo "  KOKORO_BUILD_NUMBER = ${KOKORO_BUILD_NUMBER}"

cd "${KOKORO_ARTIFACTS_DIR}/git/avf/build/debian/"
sudo losetup -D
grep vmx /proc/cpuinfo || true

# Sibling docker would be launched from host, so provide host's path for mount.
AVF_BUILD_TOP="${KOKORO_HOST_ROOT_DIR}/src/git/avf"
sudo ./build_in_container.sh -a x86_64 -b ${AVF_BUILD_TOP} -r -k -w

sudo mv images.tar.gz ${KOKORO_ARTIFACTS_DIR}
mkdir -p ${KOKORO_ARTIFACTS_DIR}/logs
sudo cp -r /var/log/fai/* ${KOKORO_ARTIFACTS_DIR}/logs || true
