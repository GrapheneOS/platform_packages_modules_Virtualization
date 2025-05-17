#!/bin/bash

set -ex

arch=""
case "${KOKORO_JOB_NAME}" in # e.g. "ferrochrome/aarch64/continuous
  *aarch64*)
    arch="aarch64"
    # Enable multi-arch container by QEMU for cros-compilation.
    sudo docker run --rm --privileged multiarch/qemu-user-static --reset -p yes
    ;;
  *x86_64*)
    arch="x86_64"
    ;;
  *)
    echo "Unexpected \${KOKORO_JOB_NAME}"
    echo "Expected to contain architecture, but was ${KOKORO_JOB_NAME}"
    exit 1
esac

cd "${KOKORO_ARTIFACTS_DIR}/git/avf/build/debian/"

# Sibling docker would be launched from host, so provide host's path for mount.
AVF_BUILD_TOP="${KOKORO_HOST_ROOT_DIR}/src/git/avf"
BUILD_ID="${KOKORO_JOB_NAME}-${KOKORO_BUILD_NUMBER}-$(date --utc)"
sudo ./build_in_container.sh -a ${arch} -t ${AVF_BUILD_TOP} -w -b "${BUILD_ID}"

sudo mv images.tar.gz ${KOKORO_ARTIFACTS_DIR}
mkdir -p ${KOKORO_ARTIFACTS_DIR}/logs
sudo cp -r /var/log/fai/* ${KOKORO_ARTIFACTS_DIR}/logs || true
