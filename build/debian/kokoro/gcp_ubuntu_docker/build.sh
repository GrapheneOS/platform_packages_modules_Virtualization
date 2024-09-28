#!/bin/bash

set -e

cd "${KOKORO_ARTIFACTS_DIR}/git/avf/build/debian/"

# FAI needs it
pyenv install 3.10
pyenv global 3.10
python --version

sudo losetup -D
sudo -E ./build.sh
