#!/bin/bash

pushd $(dirname $0) > /dev/null
tempdir=$(mktemp -d)
echo Get Debian image and dependencies...
wget https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-nocloud-amd64.raw -O ${tempdir}/debian.img
wget https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64 -O ${tempdir}/ttyd

echo Customize the image...
virt-customize --commands-from-file <(sed "s|/tmp|$tempdir|g" commands) -a ${tempdir}/debian.img

asset_dir=../assets/linux
mkdir -p ${asset_dir}

echo Copy files...

pushd ${tempdir} > /dev/null
tar czvS -f images.tar.gz debian.img
popd > /dev/null
mv ${tempdir}/images.tar.gz ${asset_dir}/images.tar.gz
cp vm_config.json ${asset_dir}

echo Calculating hash...
hash=$(cat ${asset_dir}/images.tar.gz ${asset_dir}/vm_config.json | sha1sum | cut -d' ' -f 1)
echo ${hash} > ${asset_dir}/hash

popd > /dev/null
echo Cleaning up...
rm -rf ${tempdir}