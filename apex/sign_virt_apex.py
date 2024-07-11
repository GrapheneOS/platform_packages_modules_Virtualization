#!/usr/bin/env python
#
# Copyright (C) 2021 The Android Open Source Project
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#      http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
"""sign_virt_apex is a command line tool for sign the Virt APEX file.

Typical usage:
  sign_virt_apex payload_key payload_dir
    -v, --verbose
    --verify
    --avbtool path_to_avbtool
    --signing_args args

sign_virt_apex uses external tools which are assumed to be available via PATH.
- avbtool (--avbtool can override the tool)
- lpmake, lpunpack, simg2img, img2simg, initrd_bootconfig
"""
import argparse
import binascii
import builtins
import hashlib
import os
import re
import shlex
import subprocess
import sys
import tempfile
import traceback
from concurrent import futures

# pylint: disable=line-too-long,consider-using-with

# Use executor to parallelize the invocation of external tools
# If a task depends on another, pass the future object of the previous task as wait list.
# Every future object created by a task should be consumed with AwaitAll()
# so that exceptions are propagated .
executor = futures.ThreadPoolExecutor()

# Temporary directory for unpacked super.img.
# We could put its creation/deletion into the task graph as well, but
# having it as a global setup is much simpler.
unpack_dir = tempfile.TemporaryDirectory()

# tasks created with Async() are kept in a list so that they are awaited
# before exit.
tasks = []

# create an async task and return a future value of it.
def Async(fn, *args, wait=None, **kwargs):

    # wrap a function with AwaitAll()
    def wrapped():
        AwaitAll(wait)
        fn(*args, **kwargs)

    task = executor.submit(wrapped)
    tasks.append(task)
    return task


# waits for task (captured in fs as future values) with future.result()
# so that any exception raised during task can be raised upward.
def AwaitAll(fs):
    if fs:
        for f in fs:
            f.result()


def ParseArgs(argv):
    parser = argparse.ArgumentParser(description='Sign the Virt APEX')
    parser.add_argument('--verify', action='store_true',
                        help='Verify the Virt APEX')
    parser.add_argument(
        '-v', '--verbose',
        action='store_true',
        help='verbose execution')
    parser.add_argument(
        '--avbtool',
        default='avbtool',
        help='Optional flag that specifies the AVB tool to use. Defaults to `avbtool`.')
    parser.add_argument(
        '--signing_args',
        help='the extra signing arguments passed to avbtool.'
    )
    parser.add_argument(
        '--key_override',
        metavar="filename=key",
        action='append',
        help='Overrides a signing key for a file e.g. microdroid_bootloader=mykey (for testing)')
    parser.add_argument(
        'key',
        help='path to the private key file.')
    parser.add_argument(
        'input_dir',
        help='the directory having files to be packaged')
    parser.add_argument(
        '--do_not_update_bootconfigs',
        action='store_true',
        help='This will NOT update the vbmeta related bootconfigs while signing the apex.\
            Used for testing only!!')
    parser.add_argument('--do_not_validate_avb_version', action='store_true', help='Do not validate the avb_version when updating vbmeta bootconfig. Only use in tests!')
    args = parser.parse_args(argv)
    # preprocess --key_override into a map
    args.key_overrides = {}
    if args.key_override:
        for pair in args.key_override:
            name, key = pair.split('=')
            args.key_overrides[name] = key
    return args


def RunCommand(args, cmd, env=None, expected_return_values=None):
    expected_return_values = expected_return_values or {0}
    env = env or {}
    env.update(os.environ.copy())

    # TODO(b/193504286): we need a way to find other tool (cmd[0]) in various contexts
    #  e.g. sign_apex.py, sign_target_files_apk.py
    if cmd[0] == 'avbtool':
        cmd[0] = args.avbtool

    if args.verbose:
        print('Running: ' + ' '.join(cmd))
    p = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, universal_newlines=True)
    output, _ = p.communicate()

    if args.verbose or p.returncode not in expected_return_values:
        print(output.rstrip())

    assert p.returncode in expected_return_values, (
        '%d Failed to execute: ' + ' '.join(cmd)) % p.returncode
    return (output, p.returncode)


def ReadBytesSize(value):
    return int(value.removesuffix(' bytes'))


def ExtractAvbPubkey(args, key, output):
    RunCommand(args, ['avbtool', 'extract_public_key',
               '--key', key, '--output', output])


def is_lz4(args, path):
    # error 44: Unrecognized header
    result = RunCommand(args, ['lz4', '-t', path], expected_return_values={0, 44})
    return result[1] == 0


def AvbInfo(args, image_path):
    """Parses avbtool --info image output

    Args:
      args: program arguments.
      image_path: The path to the image, either raw or lz4 compressed
      descriptor_name: Descriptor name of interest.

    Returns:
      A pair of
        - a dict that contains VBMeta info. None if there's no VBMeta info.
        - a list of descriptors.
    """
    if not os.path.exists(image_path):
        raise ValueError(f'Failed to find image: {image_path}')

    if is_lz4(args, image_path):
        with tempfile.NamedTemporaryFile() as decompressed_image:
            RunCommand(args, ['lz4', '-d', '-f', image_path, decompressed_image.name])
            return AvbInfo(args, decompressed_image.name)

    output, ret_code = RunCommand(
        args, ['avbtool', 'info_image', '--image', image_path], expected_return_values={0, 1})
    if ret_code == 1:
        return None, None

    info, descriptors = {}, []

    # Read `avbtool info_image` output as "key:value" lines
    matcher = re.compile(r'^(\s*)([^:]+):\s*(.*)$')

    def IterateLine(output):
        for line in output.split('\n'):
            line_info = matcher.match(line)
            if not line_info:
                continue
            yield line_info.group(1), line_info.group(2), line_info.group(3)

    gen = IterateLine(output)

    def ReadDescriptors(cur_indent, cur_name, cur_value):
        descriptor = cur_value if cur_name == 'Prop' else {}
        descriptors.append((cur_name, descriptor))
        for indent, key, value in gen:
            if indent <= cur_indent:
                # read descriptors recursively to pass the read key as descriptor name
                ReadDescriptors(indent, key, value)
                break
            descriptor[key] = value

    # Read VBMeta info
    for _, key, value in gen:
        if key == 'Descriptors':
            ReadDescriptors(*next(gen))
            break
        info[key] = value

    return info, descriptors


def find_all_values_by_key(pairs, key):
    """Find all the values of the key in the pairs."""
    return [v for (k, v) in pairs if k == key]

# Extract properties from the descriptors of original vbmeta image,
# append to command as parameter.
def AppendPropArgument(cmd, descriptors):
    for prop in find_all_values_by_key(descriptors, 'Prop'):
        cmd.append('--prop')
        result = re.match(r"(.+) -> '(.+)'", prop)
        cmd.append(result.group(1) + ":" + result.group(2))


def check_resigned_image_avb_info(image_path, original_info, original_descriptors, args):
    updated_info, updated_descriptors = AvbInfo(args, image_path)
    assert original_info is not None, f'no avbinfo on original image: {image_path}'
    assert updated_info is not None, f'no avbinfo on resigned image: {image_path}'
    assert_different_value(original_info, updated_info, "Public key (sha1)", image_path)
    updated_public_key = updated_info.pop("Public key (sha1)")
    if not hasattr(check_resigned_image_avb_info, "new_public_key"):
        check_resigned_image_avb_info.new_public_key = updated_public_key
    else:
        assert check_resigned_image_avb_info.new_public_key == updated_public_key, \
            "All images should be resigned with the same public key. Expected public key (sha1):" \
            f" {check_resigned_image_avb_info.new_public_key}, actual public key (sha1): " \
            f"{updated_public_key}, Path: {image_path}"
    original_info.pop("Public key (sha1)")
    assert original_info == updated_info, \
        f"Original info and updated info should be the same for {image_path}. " \
        f"Original info: {original_info}, updated info: {updated_info}"

    # Verify the descriptors of the original and updated images.
    assert len(original_descriptors) == len(updated_descriptors), \
        f"Number of descriptors should be the same for {image_path}. " \
        f"Original descriptors: {original_descriptors}, updated descriptors: {updated_descriptors}"
    original_prop_descriptors = sorted(find_all_values_by_key(original_descriptors, "Prop"))
    updated_prop_descriptors = sorted(find_all_values_by_key(updated_descriptors, "Prop"))
    assert original_prop_descriptors == updated_prop_descriptors, \
        f"Prop descriptors should be the same for {image_path}. " \
        f"Original prop descriptors: {original_prop_descriptors}, " \
        f"updated prop descriptors: {updated_prop_descriptors}"

    # Remove digest from hash descriptors before comparing, since some digests should change.
    original_hash_descriptors = extract_hash_descriptors(original_descriptors, drop_digest)
    updated_hash_descriptors = extract_hash_descriptors(updated_descriptors, drop_digest)
    assert original_hash_descriptors == updated_hash_descriptors, \
        f"Hash descriptors' parameters should be the same for {image_path}. " \
        f"Original hash descriptors: {original_hash_descriptors}, " \
        f"updated hash descriptors: {updated_hash_descriptors}"

def drop_digest(descriptor):
    return {k: v for k, v in descriptor.items() if k != "Digest"}

def AddHashFooter(args, key, image_path, additional_images=()):
    if os.path.basename(image_path) in args.key_overrides:
        key = args.key_overrides[os.path.basename(image_path)]
    info, descriptors = AvbInfo(args, image_path)
    assert info is not None, f'no avbinfo: {image_path}'

    # Extract hash descriptor of original image.
    hash_descriptors_original = extract_hash_descriptors(descriptors, drop_digest)
    for additional_image in additional_images:
        _, additional_desc = AvbInfo(args, additional_image)
        hash_descriptors = extract_hash_descriptors(additional_desc, drop_digest)
        for k, v in hash_descriptors.items():
            assert v == hash_descriptors_original[k], \
                f"Hash descriptor of {k} in {additional_image} and {image_path} should be " \
                f"the same. {additional_image}: {v}, {image_path}: {hash_descriptors_original[k]}"
            del hash_descriptors_original[k]
    assert len(hash_descriptors_original) == 1, \
        f"Only one hash descriptor is expected for {image_path} after removing " \
        f"additional images. Hash descriptors: {hash_descriptors_original}"
    [(original_image_partition_name, original_image_descriptor)] = hash_descriptors_original.items()
    assert info["Original image size"] == original_image_descriptor["Image Size"], \
        f"Original image size should be the same as the image size in the hash descriptor " \
        f"for {image_path}. Original image size: {info['Original image size']}, " \
        f"image size in the hash descriptor: {original_image_descriptor['Image Size']}"

    partition_size = str(ReadBytesSize(info['Image size']))
    algorithm = info['Algorithm']
    original_image_salt = original_image_descriptor['Salt']

    cmd = ['avbtool', 'add_hash_footer',
           '--key', key,
           '--algorithm', algorithm,
           '--partition_name', original_image_partition_name,
           '--salt', original_image_salt,
           '--partition_size', partition_size,
           '--image', image_path]
    AppendPropArgument(cmd, descriptors)
    if args.signing_args:
        cmd.extend(shlex.split(args.signing_args))
    for additional_image in additional_images:
        cmd.extend(['--include_descriptors_from_image', additional_image])
    cmd.extend(['--rollback_index', info['Rollback Index']])

    RunCommand(args, cmd)
    check_resigned_image_avb_info(image_path, info, descriptors, args)

def AddHashTreeFooter(args, key, image_path):
    if os.path.basename(image_path) in args.key_overrides:
        key = args.key_overrides[os.path.basename(image_path)]
    info, descriptors = AvbInfo(args, image_path)
    if info:
        descriptor = find_all_values_by_key(descriptors, 'Hashtree descriptor')[0]
        image_size = ReadBytesSize(info['Image size'])
        algorithm = info['Algorithm']
        partition_name = descriptor['Partition Name']
        hash_algorithm = descriptor['Hash Algorithm']
        salt = descriptor['Salt']
        partition_size = str(image_size)
        cmd = ['avbtool', 'add_hashtree_footer',
               '--key', key,
               '--algorithm', algorithm,
               '--partition_name', partition_name,
               '--partition_size', partition_size,
               '--do_not_generate_fec',
               '--hash_algorithm', hash_algorithm,
               '--salt', salt,
               '--image', image_path]
        AppendPropArgument(cmd, descriptors)
        if args.signing_args:
            cmd.extend(shlex.split(args.signing_args))
        RunCommand(args, cmd)
        check_resigned_image_avb_info(image_path, info, descriptors, args)


def UpdateVbmetaBootconfig(args, initrds, vbmeta_img):
    # Update the bootconfigs in ramdisk
    def detach_bootconfigs(initrd_bc, initrd, bc):
        cmd = ['initrd_bootconfig', 'detach', initrd_bc, initrd, bc]
        RunCommand(args, cmd)

    def attach_bootconfigs(initrd_bc, initrd, bc):
        cmd = ['initrd_bootconfig', 'attach',
               initrd, bc, '--output', initrd_bc]
        RunCommand(args, cmd)

    # Validate that avb version used while signing the apex is the same as used by build server
    def validate_avb_version(bootconfigs):
        cmd = ['avbtool', 'version']
        stdout, _ = RunCommand(args, cmd)
        avb_version_curr = stdout.split(" ")[1].strip()
        avb_version_curr = avb_version_curr[0:avb_version_curr.rfind('.')]

        avb_version_bc = re.search(
            r"androidboot.vbmeta.avb_version = \"([^\"]*)\"", bootconfigs).group(1)
        if avb_version_curr != avb_version_bc:
            raise builtins.Exception(f'AVB version mismatch between current & one & \
                used to build bootconfigs:{avb_version_curr}&{avb_version_bc}')

    def calc_vbmeta_digest():
        cmd = ['avbtool', 'calculate_vbmeta_digest', '--image',
               vbmeta_img, '--hash_algorithm', 'sha256']
        stdout, _ = RunCommand(args, cmd)
        return stdout.strip()

    def calc_vbmeta_size():
        cmd = ['avbtool', 'info_image', '--image', vbmeta_img]
        stdout, _ = RunCommand(args, cmd)
        size = 0
        for line in stdout.split("\n"):
            line = line.split(":")
            if line[0] in ['Header Block', 'Authentication Block', 'Auxiliary Block']:
                size += int(line[1].strip()[0:-6])
        return size

    def update_vbmeta_digest(bootconfigs):
        # Update androidboot.vbmeta.digest in bootconfigs
        result = re.search(
            r"androidboot.vbmeta.digest = \"[^\"]*\"", bootconfigs)
        if not result:
            raise ValueError("Failed to find androidboot.vbmeta.digest")

        return bootconfigs.replace(result.group(),
                                   f'androidboot.vbmeta.digest = "{calc_vbmeta_digest()}"')

    def update_vbmeta_size(bootconfigs):
        # Update androidboot.vbmeta.size in bootconfigs
        result = re.search(r"androidboot.vbmeta.size = [0-9]+", bootconfigs)
        if not result:
            raise ValueError("Failed to find androidboot.vbmeta.size")
        return bootconfigs.replace(result.group(),
                                   f'androidboot.vbmeta.size = {calc_vbmeta_size()}')

    with tempfile.TemporaryDirectory() as work_dir:
        tmp_initrd = os.path.join(work_dir, 'initrd')
        tmp_bc = os.path.join(work_dir, 'bc')

        for initrd in initrds:
            detach_bootconfigs(initrd, tmp_initrd, tmp_bc)
            bc_file = open(tmp_bc, "rt", encoding="utf-8")
            bc_data = bc_file.read()
            if not args.do_not_validate_avb_version:
                validate_avb_version(bc_data)
            bc_data = update_vbmeta_digest(bc_data)
            bc_data = update_vbmeta_size(bc_data)
            bc_file.close()
            bc_file = open(tmp_bc, "wt", encoding="utf-8")
            bc_file.write(bc_data)
            bc_file.flush()
            attach_bootconfigs(initrd, tmp_initrd, tmp_bc)


def MakeVbmetaImage(args, key, vbmeta_img, images=None, chained_partitions=None):
    if os.path.basename(vbmeta_img) in args.key_overrides:
        key = args.key_overrides[os.path.basename(vbmeta_img)]
    info, descriptors = AvbInfo(args, vbmeta_img)
    if info is None:
        return

    with tempfile.TemporaryDirectory() as work_dir:
        algorithm = info['Algorithm']
        rollback_index = info['Rollback Index']
        rollback_index_location = info['Rollback Index Location']

        cmd = ['avbtool', 'make_vbmeta_image',
               '--key', key,
               '--algorithm', algorithm,
               '--rollback_index', rollback_index,
               '--rollback_index_location', rollback_index_location,
               '--output', vbmeta_img]
        if images:
            for img in images:
                cmd.extend(['--include_descriptors_from_image', img])

        # replace pubkeys of chained_partitions as well
        for name, descriptor in descriptors:
            if name == 'Chain Partition descriptor':
                part_name = descriptor['Partition Name']
                ril = descriptor['Rollback Index Location']
                part_key = chained_partitions[part_name]
                avbpubkey = os.path.join(work_dir, part_name + '.avbpubkey')
                ExtractAvbPubkey(args, part_key, avbpubkey)
                cmd.extend(['--chain_partition', f'{part_name}:{ril}:{avbpubkey}'])

        if args.signing_args:
            cmd.extend(shlex.split(args.signing_args))

        RunCommand(args, cmd)
        check_resigned_image_avb_info(vbmeta_img, info, descriptors, args)
        # libavb expects to be able to read the maximum vbmeta size, so we must provide a partition
        # which matches this or the read will fail.
        with open(vbmeta_img, 'a', encoding='utf8') as f:
            f.truncate(65536)


def UnpackSuperImg(args, super_img, work_dir):
    tmp_super_img = os.path.join(work_dir, 'super.img')
    RunCommand(args, ['simg2img', super_img, tmp_super_img])
    RunCommand(args, ['lpunpack', tmp_super_img, work_dir])


def MakeSuperImage(args, partitions, output):
    with tempfile.TemporaryDirectory() as work_dir:
        cmd = ['lpmake', '--device-size=auto', '--metadata-slots=2',  # A/B
               '--metadata-size=65536', '--sparse', '--output=' + output]

        for part, img in partitions.items():
            tmp_img = os.path.join(work_dir, part)
            RunCommand(args, ['img2simg', img, tmp_img])

            image_arg = f'--image={part}={img}'
            partition_arg = f'--partition={part}:readonly:{os.path.getsize(img)}:default'
            cmd.extend([image_arg, partition_arg])

        RunCommand(args, cmd)


def GenVbmetaImage(args, image, output, partition_name, salt):
    cmd = ['avbtool', 'add_hash_footer', '--dynamic_partition_size',
           '--do_not_append_vbmeta_image',
           '--partition_name', partition_name,
           '--salt', salt,
           '--image', image,
           '--output_vbmeta_image', output]
    RunCommand(args, cmd)


gki_versions = ['android15-6.6']

# dict of (key, file) for re-sign/verification. keys are un-versioned for readability.
virt_apex_non_gki_files = {
    'kernel': 'etc/fs/microdroid_kernel',
    'vbmeta.img': 'etc/fs/microdroid_vbmeta.img',
    'super.img': 'etc/fs/microdroid_super.img',
    'initrd_normal.img': 'etc/microdroid_initrd_normal.img',
    'initrd_debuggable.img': 'etc/microdroid_initrd_debuggable.img',
    'rialto': 'etc/rialto.bin',
}

def TargetFiles(input_dir):
    ret = {k: os.path.join(input_dir, v) for k, v in virt_apex_non_gki_files.items()}

    for ver in gki_versions:
        kernel        = os.path.join(input_dir, f'etc/fs/microdroid_gki-{ver}_kernel')
        initrd_normal = os.path.join(input_dir, f'etc/microdroid_gki-{ver}_initrd_normal.img')
        initrd_debug  = os.path.join(input_dir, f'etc/microdroid_gki-{ver}_initrd_debuggable.img')

        if os.path.isfile(kernel):
            ret[f'gki-{ver}_kernel']                = kernel
            ret[f'gki-{ver}_initrd_normal.img']     = initrd_normal
            ret[f'gki-{ver}_initrd_debuggable.img'] = initrd_debug

    return ret

def IsInitrdImage(path):
    return path.endswith('initrd_normal.img') or path.endswith('initrd_debuggable.img')


def SignVirtApex(args):
    key = args.key
    input_dir = args.input_dir
    files = TargetFiles(input_dir)

    # unpacked files (will be unpacked from super.img below)
    system_a_img = os.path.join(unpack_dir.name, 'system_a.img')
    vendor_a_img = os.path.join(unpack_dir.name, 'vendor_a.img')

    # re-sign super.img
    # 1. unpack super.img
    # 2. resign system and vendor (if exists)
    # 3. repack super.img out of resigned system and vendor (if exists)
    UnpackSuperImg(args, files['super.img'], unpack_dir.name)
    system_a_f = Async(AddHashTreeFooter, args, key, system_a_img)
    partitions = {"system_a": system_a_img}
    images = [system_a_img]
    images_f = [system_a_f]

    # if vendor_a.img exists, resign it
    if os.path.exists(vendor_a_img):
        partitions.update({'vendor_a': vendor_a_img})
        images.append(vendor_a_img)
        vendor_a_f = Async(AddHashTreeFooter, args, key, vendor_a_img)
        images_f.append(vendor_a_f)

    Async(MakeSuperImage, args, partitions,
          files['super.img'], wait=images_f)

    # re-generate vbmeta from re-signed system_a.img
    vbmeta_f = Async(MakeVbmetaImage, args, key, files['vbmeta.img'],
                     images=images,
                     wait=images_f)

    vbmeta_bc_f = None
    if not args.do_not_update_bootconfigs:
        initrd_files = [v for k, v in files.items() if IsInitrdImage(k)]
        vbmeta_bc_f = Async(UpdateVbmetaBootconfig, args, initrd_files,
                            files['vbmeta.img'],
                            wait=[vbmeta_f])

    # Re-sign kernel. Note kernel's vbmeta contain addition descriptor from ramdisk(s)
    def resign_decompressed_kernel(kernel_file, initrd_normal_file, initrd_debug_file):
        _, kernel_image_descriptors = AvbInfo(args, kernel_file)
        salts = extract_hash_descriptors(
            kernel_image_descriptors, lambda descriptor: descriptor['Salt'])
        initrd_normal_hashdesc = tempfile.NamedTemporaryFile(delete=False).name
        initrd_debug_hashdesc = tempfile.NamedTemporaryFile(delete=False).name
        initrd_n_f = Async(GenVbmetaImage, args, initrd_normal_file,
                           initrd_normal_hashdesc, "initrd_normal", salts["initrd_normal"],
                           wait=[vbmeta_bc_f] if vbmeta_bc_f is not None else [])
        initrd_d_f = Async(GenVbmetaImage, args, initrd_debug_file,
                           initrd_debug_hashdesc, "initrd_debug", salts["initrd_debug"],
                           wait=[vbmeta_bc_f] if vbmeta_bc_f is not None else [])
        return Async(AddHashFooter, args, key, kernel_file,
              additional_images=[initrd_normal_hashdesc, initrd_debug_hashdesc],
              wait=[initrd_n_f, initrd_d_f])

    def resign_compressed_kernel(kernel_file, initrd_normal_file, initrd_debug_file):
        # decompress, re-sign, compress again
        with tempfile.TemporaryDirectory() as work_dir:
            decompressed_kernel_file = os.path.join(work_dir, os.path.basename(kernel_file))
            RunCommand(args, ['lz4', '-d', kernel_file, decompressed_kernel_file])
            resign_decompressed_kernel(decompressed_kernel_file, initrd_normal_file,
                                       initrd_debug_file).result()
            RunCommand(args, ['lz4', '-9', '-f', decompressed_kernel_file, kernel_file])

    def resign_kernel(kernel, initrd_normal, initrd_debug):
        kernel_file = files[kernel]
        initrd_normal_file = files[initrd_normal]
        initrd_debug_file = files[initrd_debug]

        # kernel may be compressed with lz4.
        if is_lz4(args, kernel_file):
            return Async(resign_compressed_kernel, kernel_file, initrd_normal_file,
                         initrd_debug_file)
        else:
            return resign_decompressed_kernel(kernel_file, initrd_normal_file, initrd_debug_file)

    _, original_kernel_descriptors = AvbInfo(args, files['kernel'])
    resign_kernel_tasks = [resign_kernel('kernel', 'initrd_normal.img', 'initrd_debuggable.img')]
    original_kernels = {"kernel" : original_kernel_descriptors}

    for ver in gki_versions:
        if f'gki-{ver}_kernel' in files:
            kernel_name = f'gki-{ver}_kernel'
            _, original_kernel_descriptors = AvbInfo(args, files[kernel_name])
            task = resign_kernel(
                kernel_name,
                f'gki-{ver}_initrd_normal.img',
                f'gki-{ver}_initrd_debuggable.img')
            resign_kernel_tasks.append(task)
            original_kernels[kernel_name] = original_kernel_descriptors

    # Re-sign rialto if it exists. Rialto only exists in arm64 environment.
    if os.path.exists(files['rialto']):
        update_initrd_digests_task = Async(
            update_initrd_digests_of_kernels_in_rialto, original_kernels, args, files,
            wait=resign_kernel_tasks)
        Async(resign_rialto, args, key, files['rialto'], wait=[update_initrd_digests_task])

def resign_rialto(args, key, rialto_path):
    _, original_descriptors = AvbInfo(args, rialto_path)
    AddHashFooter(args, key, rialto_path)

    # Verify the new AVB footer.
    updated_info, updated_descriptors = AvbInfo(args, rialto_path)
    assert len(updated_descriptors) == 2, \
        f"There should be two descriptors for rialto. Updated descriptors: {updated_descriptors}"
    updated_prop = find_all_values_by_key(updated_descriptors, "Prop")
    assert len(updated_prop) == 1, "There should be only one Prop descriptor for rialto. " \
        f"Updated descriptors: {updated_descriptors}"
    assert updated_info["Rollback Index"] != "0", "Rollback index should not be zero for rialto."

    # Verify the only hash descriptor of rialto.
    updated_hash_descriptors = extract_hash_descriptors(updated_descriptors)
    assert len(updated_hash_descriptors) == 1, \
        f"There should be only one hash descriptor for rialto. " \
        f"Updated hash descriptors: {updated_hash_descriptors}"
    # Since salt is not updated, the change of digest reflects the change of content of rialto
    # kernel.
    if not args.do_not_update_bootconfigs:
        [(_, original_descriptor)] = extract_hash_descriptors(original_descriptors).items()
        [(_, updated_descriptor)] = updated_hash_descriptors.items()
        assert_different_value(original_descriptor, updated_descriptor, "Digest",
                               "rialto_hash_descriptor")

def assert_different_value(original, updated, key, context):
    assert original[key] != updated[key], \
        f"Value of '{key}' should change for '{context}'" \
        f"Original value: {original[key]}, updated value: {updated[key]}"

def update_initrd_digests_of_kernels_in_rialto(original_kernels, args, files):
    # Update the hashes of initrd_normal and initrd_debug in rialto if the
    # bootconfigs in them are updated.
    if args.do_not_update_bootconfigs:
        return

    with open(files['rialto'], "rb") as file:
        content = file.read()

    for kernel_name, descriptors in original_kernels.items():
        content = update_initrd_digests_in_rialto(
            descriptors, args, files, kernel_name, content)

    with open(files['rialto'], "wb") as file:
        file.write(content)

def update_initrd_digests_in_rialto(
        original_descriptors, args, files, kernel_name, content):
    _, updated_descriptors = AvbInfo(args, files[kernel_name])

    original_digests = extract_hash_descriptors(
        original_descriptors, lambda x: binascii.unhexlify(x['Digest']))
    updated_digests = extract_hash_descriptors(
        updated_descriptors, lambda x: binascii.unhexlify(x['Digest']))
    assert original_digests.pop("boot") == updated_digests.pop("boot"), \
        "Hash descriptor of boot should not change for " + kernel_name + \
        f"\nOriginal descriptors: {original_descriptors}, " \
        f"\nUpdated descriptors: {updated_descriptors}"

    # Check that the original and updated digests are different before updating rialto.
    partition_names = {'initrd_normal', 'initrd_debug'}
    assert set(original_digests.keys()) == set(updated_digests.keys()) == partition_names, \
        f"Original digests' partitions should be {partition_names}. " \
        f"Original digests: {original_digests}. Updated digests: {updated_digests}"
    assert set(original_digests.values()).isdisjoint(updated_digests.values()), \
        "Digests of initrd_normal and initrd_debug should change. " \
        f"Original descriptors: {original_descriptors}, " \
        f"updated descriptors: {updated_descriptors}"

    for partition_name, original_digest in original_digests.items():
        updated_digest = updated_digests[partition_name]
        assert len(original_digest) == len(updated_digest), \
            f"Length of original_digest and updated_digest must be the same for {partition_name}." \
            f" Original digest: {original_digest}, updated digest: {updated_digest}"

        new_content = content.replace(original_digest, updated_digest)
        assert len(new_content) == len(content), \
            "Length of new_content and content must be the same."
        assert new_content != content, \
            f"original digest of the partition {partition_name} not found."
        content = new_content

    return content

def extract_hash_descriptors(descriptors, f=lambda x: x):
    return {desc["Partition Name"]: f(desc) for desc in
            find_all_values_by_key(descriptors, "Hash descriptor")}

def VerifyVirtApex(args):
    key = args.key
    input_dir = args.input_dir
    files = TargetFiles(input_dir)

    # unpacked files
    UnpackSuperImg(args, files['super.img'], unpack_dir.name)
    system_a_img = os.path.join(unpack_dir.name, 'system_a.img')

    # Read pubkey digest from the input key
    with tempfile.NamedTemporaryFile() as pubkey_file:
        ExtractAvbPubkey(args, key, pubkey_file.name)
        with open(pubkey_file.name, 'rb') as f:
            pubkey = f.read()
            pubkey_digest = hashlib.sha1(pubkey).hexdigest()

    def check_avb_pubkey(file):
        info, _ = AvbInfo(args, file)
        assert info is not None, f'no avbinfo: {file}'
        assert info['Public key (sha1)'] == pubkey_digest, f'pubkey mismatch: {file}'

    for k, f in files.items():
        if IsInitrdImage(k):
            # TODO(b/245277660): Verify that ramdisks contain the correct vbmeta digest
            continue
        if k == 'rialto' and not os.path.exists(f):
            # Rialto only exists in arm64 environment.
            continue
        if k == 'super.img':
            Async(check_avb_pubkey, system_a_img)
        else:
            # Check pubkey for other files using avbtool
            Async(check_avb_pubkey, f)


def main(argv):
    try:
        args = ParseArgs(argv)
        if args.verify:
            VerifyVirtApex(args)
        else:
            SignVirtApex(args)
        # ensure all tasks are completed without exceptions
        AwaitAll(tasks)
    except:  # pylint: disable=bare-except
        traceback.print_exc()
        sys.exit(1)


if __name__ == '__main__':
    main(sys.argv[1:])
