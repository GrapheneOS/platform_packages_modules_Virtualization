"""Extract information about vbmeta (digest/size) required in (androidboot.*)
bootconfig. It uses avbtool to find some values such as vbmeta size, digest"""
#!/usr/bin/env python3

import sys
import subprocess

def main(args):
    """Runs avbtool to generate vbmeta related bootconfigs"""
    avbtool = args[0]
    vbmeta_img = args[1]
    hash_algorithm = 'sha256'
    size = 0

    with subprocess.Popen([avbtool, 'version'],
                            stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT) as proc:
        stdout, _ = proc.communicate()
        avb_version = stdout.decode("utf-8").split(" ")[1].strip()
        avb_version = avb_version[0:avb_version.rfind('.')]

    with subprocess.Popen([avbtool, 'info_image', '--image', vbmeta_img],
                           stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT) as proc:
        stdout, _ = proc.communicate()
        for line in stdout.decode("utf-8").split("\n"):
            line = line.split(":")
            if line[0] in \
                ['Header Block', 'Authentication Block', 'Auxiliary Block']:
                size += int(line[1].strip()[0:-6])

    with subprocess.Popen([avbtool, 'calculate_vbmeta_digest',
                            '--image', vbmeta_img,
                            '--hash_algorithm', hash_algorithm],
                           stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT) as proc:
        stdout, _ = proc.communicate()
        vbmeta_hash = stdout.decode("utf-8").strip()

    print(f'androidboot.vbmeta.size = {size}')
    print(f'androidboot.vbmeta.digest = \"{vbmeta_hash}\"')
    print(f'androidboot.vbmeta.hash_alg = \"{hash_algorithm}\"')
    print(f'androidboot.vbmeta.avb_version = \"{avb_version}\"')
    print('androidboot.veritymode = "enforcing"')
    print('androidboot.vbmeta.invalidate_on_error = "yes"')
    print('androidboot.vbmeta.device_state = "locked"')
    print('androidboot.vbmeta.device = "/dev/block/by-name/vbmeta_a"')
    print('androidboot.slot_suffix = "_a"')
    print('androidboot.force_normal_boot = "1"')
    print('androidboot.verifiedbootstate = "green"')

## Main body
if __name__ == '__main__':
    main(sys.argv[1:])
