#!/bin/bash

set -ex

cd "${KOKORO_ARTIFACTS_DIR}/git/avf/build/debian/"
sudo losetup -D
grep vmx /proc/cpuinfo || true

# Sibling docker would be launched from host, so provide host's path for mount.
AVF_BUILD_TOP="${KOKORO_HOST_ROOT}/src/git/avf"
sudo ./build_in_container.sh -a aarch64 -b ${AVF_BUILD_TOP} -r -k \
  -i us-central1-docker.pkg.dev/kokoro-container-bakery/kokoro/ubuntu/ubuntu2204/full:current

sudo mv images.tar.gz ${KOKORO_ARTIFACTS_DIR} || true
mkdir -p ${KOKORO_ARTIFACTS_DIR}/logs
sudo cp -r /var/log/fai/* ${KOKORO_ARTIFACTS_DIR}/logs || true
