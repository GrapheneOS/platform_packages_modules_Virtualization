/*
 * Copyright 2026 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.android.microdroid.test.preparer;

import com.android.tradefed.config.Option;
import com.android.tradefed.invoker.IInvocationContext;
import com.android.tradefed.log.LogUtil.CLog;
import com.android.tradefed.testtype.suite.module.BaseModuleController;
import com.android.tradefed.util.AaptParser;
import com.android.tradefed.util.AbiUtils;
import com.android.tradefed.util.SearchArtifactUtil;

import java.io.File;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.Set;

/**
 * Module controller to not run tests when installation would fail due to the specified `--abi`
 * mismatch with test apk's native code.
 *
 * <p>This is a workaround for GTS where whole test suites are declared as 'supports all arches'.
 */
public class TestInstallAbiModuleController extends BaseModuleController {

    @Option(
            name = "test-apk-name",
            description = "The test apk file name to be validated",
            mandatory = true)
    private String mTestApkName;

    @Override
    public RunStrategy shouldRun(IInvocationContext context) {
        // `adb install --abi ${moduleArch}` will be passed by SuiteApkInstaller.
        String moduleAbi = getModuleAbi().getName();
        String moduleArch = AbiUtils.getArchForAbi(moduleAbi);

        File testApk = SearchArtifactUtil.searchFile(mTestApkName, /* targetFirst= */ false);
        AaptParser apkInfo = AaptParser.parse(testApk);

        Set<String> apkArches = new HashSet<>();
        for (String appAbi : apkInfo.getNativeCode()) {
            apkArches.add(AbiUtils.getArchForAbi(appAbi));
        }

        if (!apkArches.contains(moduleArch)) {
            CLog.w(
                    "Skipping module %s running as abi %s. %s's native code "
                            + "only supports %s, so installation would fail.",
                    getModuleName(), moduleAbi, mTestApkName, apkArches);
            return RunStrategy.FULL_MODULE_BYPASS;
        }
        return RunStrategy.RUN;
    }
}
