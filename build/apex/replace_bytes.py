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
"""replace_bytes is a command line tool to replace bytes in a file.

Typical usage: replace_bytes target_file old_file new_file

  replace bytes of old_file with bytes of new_file in target_file. old_file and new_file should be
  the same size.

"""
import argparse
import sys


def ParseArgs(argv):
    parser = argparse.ArgumentParser(description='Replace bytes')
    parser.add_argument(
        'target_file',
        help='path to the target file.')
    parser.add_argument(
        'old_file',
        help='path to the file containing old bytes')
    parser.add_argument(
        'new_file',
        help='path to the file containing new bytes')
    return parser.parse_args(argv)


def ReplaceBytes(target_file, old_file, new_file):
    # read old bytes
    with open(old_file, 'rb') as f:
        old_bytes = f.read()

    # read new bytes
    with open(new_file, 'rb') as f:
        new_bytes = f.read()

    assert len(old_bytes) == len(new_bytes), 'Pubkeys should be the same size. (%d != %d)' % (
        len(old_bytes), len(new_bytes))

    # replace bytes in target_file
    with open(target_file, 'r+b') as f:
        pos = f.read().find(old_bytes)
        assert pos != -1, 'Pubkey not found'
        f.seek(pos)
        f.write(new_bytes)


def main(argv):
    try:
        args = ParseArgs(argv)
        ReplaceBytes(args.target_file, args.old_file, args.new_file)
    except Exception as e:
        print(e)
        sys.exit(1)


if __name__ == '__main__':
    main(sys.argv[1:])
