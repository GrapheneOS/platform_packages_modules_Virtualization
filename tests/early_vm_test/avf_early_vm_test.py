#!/usr/bin/env python3
#
# Copyright 2025 The Android Open Source Project
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
#

import logging
import os
import subprocess
import unittest

_DEFAULT_COMMAND_TIMEOUT = 300
_LAUNCHER_PATH = "/system_ext/bin/avf_early_vm_test_launcher"
_RIALTO_PATH = "/system_ext/etc/avf/rialto_test.bin"

def _RunCommand(cmd, timeout=_DEFAULT_COMMAND_TIMEOUT):
    with subprocess.Popen(args=cmd,
                          stderr=subprocess.PIPE,
                          stdout=subprocess.PIPE,
                          universal_newlines=True) as proc:
        try:
            out, err = proc.communicate(timeout=timeout)
            returncode = proc.returncode
        except subprocess.TimeoutExpired:
            proc.kill()
            out, err = proc.communicate()
            returncode = proc.returncode

    return out, err, returncode

class AvfEarlyVmTest(unittest.TestCase):
    def setUp(self):
        self._serial_number = os.environ.get("ANDROID_SERIAL")
        self.assertTrue(self._serial_number, "$ANDROID_SERIAL is empty.")

    def _TestAvfEarlyVm(self, protected):
        adb_cmd = ["adb", "-s", self._serial_number, "shell", _LAUNCHER_PATH, "--kernel",
                   _RIALTO_PATH]
        if protected:
            adb_cmd.append("--protected")

        _, err, returncode = _RunCommand(adb_cmd)
        self.assertEqual(returncode, 0, f"{adb_cmd} failed: {err}")

    def testAvfEarlyVmNonProtected(self):
        self._TestAvfEarlyVm(False)

    def testAvfEarlyVmProtected(self):
        self._TestAvfEarlyVm(True)

if __name__ == "__main__":
    # Setting verbosity is required to generate output that the TradeFed test
    # runner can parse.
    unittest.main(verbosity=3)
