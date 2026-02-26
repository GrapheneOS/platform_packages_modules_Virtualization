#!/bin/bash

if [[ "${1}" == "-h" || "${1}" == "--help" || -n "${2}" ]]; then
  echo "fetch-cidata.sh [build_id]"
  echo ""
  echo "   Fetch cidata.sh from build server with the specified build id or lkgb"
  exit 0
fi

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

set -e

AB="/google/bin/releases/android/ab/ab.par"
FETCH="/google/data/ro/projects/android/fetch_artifact"
BRANCH="git_main-throttled-nightly"
TARGET="cf_arm64_only_phone-trunk_staging-userdebug_musl"
BID="${1}"

if [[ -z "${BID}" ]]; then
  BID=$(${AB} lkgb --branch ${BRANCH} --target ${TARGET} | awk '{print $3}')
fi

echo "Fetch with bid=${BID}"

ARCHS=(arm64 x86_64)

for arch in "${ARCHS[@]}"; do
  echo "Fetching for ${arch}"

  ${FETCH} --bid ${BID} --target "${TARGET}" "debian_cidata_${arch}.iso" "${SCRIPT_DIR}/assets-${arch}/cidata.iso"
  echo ${BID} > "${SCRIPT_DIR}/assets-${arch}/cidata.build_id"
done

echo "DONE"
