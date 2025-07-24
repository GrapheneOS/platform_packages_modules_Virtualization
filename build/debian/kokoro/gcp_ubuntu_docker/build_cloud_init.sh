#!/bin/bash

set -ex

SCRIPT_DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

${SCRIPT_DIR}/build.sh -c
