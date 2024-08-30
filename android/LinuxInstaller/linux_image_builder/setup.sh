#!/bin/bash
wget https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-nocloud-arm64.raw -O debian.img
wget  https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.aarch64 -O ttyd
virt-customize --commands-from-file commands -a debian.img