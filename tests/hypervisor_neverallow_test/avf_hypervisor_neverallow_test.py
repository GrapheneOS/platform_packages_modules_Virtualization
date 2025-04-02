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
import pkgutil
import shutil
import subprocess
import tempfile
import unittest

_DEFAULT_COMMAND_TIMEOUT = 300

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


class AvfHypervisorNeverallowTest(unittest.TestCase):
    """Neverallow Rules SELinux tests to ensure hypervisor devices can only be
    used by crosvm.

    This test finds the security context of all supported hypervisor device
    files, generates a neverallow rule for each of them, and ensures the
    device's policy does not violate those neverallows.

    The more general SELinuxNeverallowRulesTest is enough to ensure this for
    KVM, but the other hypervisors are labelled by vendor policies and so
    require this roundabout technique.
    """

    def setUp(self):
        self._serial_number = os.environ.get("ANDROID_SERIAL")
        self.assertTrue(self._serial_number, "$ANDROID_SERIAL is empty.")
        self._temp_dir = tempfile.mkdtemp()

        analyzer = "sepolicy-analyze"
        analyzer_path = os.path.join(self._temp_dir, analyzer)
        with open(analyzer_path, "wb") as f:
            blob = pkgutil.get_data("avf_hypervisor_neverallow_test", analyzer)
            self.assertTrue(blob,
                            f"Error: {analyzer} does not exist. Is this binary "
                            "corrupted?\n")
            f.write(blob)
        self.assertEqual(_RunCommand(["chmod", "+x", analyzer_path])[2], 0,
                         "failed to set +x")

        self._analyzer_path = analyzer_path

    def tearDown(self):
        shutil.rmtree(self._temp_dir)

    def _runAdbCommand(self, cmd):
        adb_cmd = ["adb", "-s", self._serial_number] + cmd
        return _RunCommand(adb_cmd)

    def _checkAdbCommandOutput(self, cmd):
        out, err, returncode = self._runAdbCommand(cmd)
        self.assertEqual(returncode, 0, f"adb '{cmd}' failed: {err}")

        return out

    def _getProp(self, prop):
        return self._checkAdbCommandOutput(["shell", "getprop", prop]).strip()

    def _pullDevicePolicy(self, policy_path):
        self._checkAdbCommandOutput(["pull", "/sys/fs/selinux/policy",
                                     policy_path])

    def _getFirstApiLevel(self):
        # Copied from Java's PropertyUtil.getFirstApiLevel
        first_api_level = self._getProp("ro.product.first_api_level")
        if first_api_level:
            try:
                return int(first_api_level)
            except ValueError:
                logging.warning("can't parse ro.product.first_api_level: %s",
                                first_api_level)

        sdk_version = self._getProp("ro.build.version.sdk")
        if sdk_version:
            try:
                return int(sdk_version)
            except ValueError:
                logging.warning("can't parse ro.build.version.sdk: %s",
                                sdk_version)

        return -1

    def _testNeverallowRule(self, policy_path, rule, path):
        # We use `--warn` to ensure that rule is valid, e.g. the context must
        # be defined in the policy, which is important in case the context
        # parsing code breaks. Since we get the context from the device, it is
        # reasonable to require it to exist.
        cmd = [self._analyzer_path, policy_path, "neverallow", "--warn", "-n", rule]
        out, err, returncode = _RunCommand(cmd)
        self.assertTrue(
            returncode == 0 and 'Warning!' not in err,
            "The following errors or warnings were encountered when validating the SELinux "
            f"neverallow rule for {path}:\n{rule}\nreturncode: {returncode}\nstdout:\n{out}\n"
            f"stderr:\n{err}\n")

    def _isVmSupported(self):
        vm_supported = self._getProp("ro.boot.hypervisor.vm.supported")
        if vm_supported in ("1", "true"):
            return True

        pvm_supported = self._getProp(
            "ro.boot.hypervisor.protected_vm.supported")
        if pvm_supported in ("1", "true"):
            return True

        return False

    def testHypervisorNeverallowTest(self):
        if not self._isVmSupported():
            logging.info("Skip test where VMs are not supported")
            return

        policy_path = os.path.join(self._temp_dir, "device_policy")
        self._pullDevicePolicy(policy_path)

        # Hypervisors supported by AVF.
        hypervisors = ["/dev/kvm", "/dev/gunyah", "/dev/gz"]

        # There are devices that launched with Android <= 15 that use alternate
        # paths, like /dev/qgunyah, from outside crosvm and AVF in their vendor
        # policies. Forbid it on newer devices.
        if self._getFirstApiLevel() >= 36:
            hypervisors.append("/dev/*gunyah")

        # Get the security context for the devices.

        # We don't check the exit code because not all of the files will exist.
        # Instead we require that there is at least one result, which must be
        # the case because the device advertised VM support.
        out, err, _ = self._runAdbCommand(["shell", "ls", "-Z"] + hypervisors)
        out = out.strip()
        self.assertTrue(out,
            "Failed to find security context for hypervisor device in `ls` "
            f"output. err: {err}")

        # `ls` outputs looks like `u:object_r:kvm_device:s0 /dev/kvm`.
        for line in out.split('\n'):
            try:
                context = line.split(':')[2]
                path = line.split()[-1]
            except IndexError:
                self.fail(f"Failed to parse: {line}")
            rule = "neverallow {domain -crosvm} " f"{context}:chr_file " \
                   "{open ioctl read write};"
            self._testNeverallowRule(policy_path, rule, path)

if __name__ == "__main__":
    # Setting verbosity is required to generate output that the TradeFed test
    # runner can parse.
    unittest.main(verbosity=3)
