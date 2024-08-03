#!/bin/bash
# Repacks chromiumos_*.bin into the assets of FerrochromeApp

usage() {
	echo "Usage: $0 CHROME_OS_DISK_IMAGE"
	exit 1
}

if [ "$#" -ne 1 ]; then
	usage
fi

disk=$1

loop=$(sudo losetup --show -f -P ${disk})
kern=$(sudo fdisk -x ${loop} | grep KERN-A | awk "{print\$1}")
root=$(sudo fdisk -x ${loop} | grep ROOT-A | awk "{print\$1}")
efi=$(sudo fdisk -x ${loop} | grep EFI-SYSTEM | awk "{print\$1}")
state=$(sudo fdisk -x ${loop} | grep STATE | awk "{print\$1}")
root_guid=$(sudo fdisk -x ${loop} | grep ROOT-A | awk "{print\$6}")

tempdir=$(mktemp -d)
pushd ${tempdir} > /dev/null
echo Extracting partition images...
sudo cp --sparse=always ${kern} kernel.img
sudo cp --sparse=always ${root} root.img
sudo cp --sparse=always ${efi} efi.img
sudo cp --sparse=always ${state} state.img
sudo chmod 777 *.img

echo Archiving. This can take long...
tar czvS -f images.tar.gz *.img

echo Splitting...
split -b 100M -d images.tar.gz images.tar.gz.part

popd > /dev/null
asset_dir=$(dirname $0)/assets/ferrochrome
echo Updating ${asset_dir}...
vm_config_template=$(dirname $0)/vm_config.json.template
mkdir -p ${asset_dir}
rm ${asset_dir}/images.tar.gz.part*
mv ${tempdir}/images.tar.gz.part* ${asset_dir}
sed -E s/GUID/${root_guid}/ ${vm_config_template} > ${asset_dir}/vm_config.json

echo Calculating hash...
hash=$(cat ${tempdir}/images.tar.gz ${asset_dir}/vm_config.json | sha1sum | cut -d' ' -f 1)
echo ${hash} > ${asset_dir}/version

echo Cleanup...
sudo losetup -d ${loop}
rm -rf ${tempdir}
echo Done.
