/*
 * Copyright 2023 The Android Open Source Project
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

package com.android.pvmfw;

import com.android.pvmfw.test.host.Pvmfw;

import java.io.File;
import java.io.IOException;

/** CLI for {@link com.android.microdroid.test.host.Pvmfw}. */
public class PvmfwTool {
    public static void printUsage() {
        System.out.println("pvmfw-tool: Appends pvmfw.bin and config payloads.");
        System.out.println("            Requires BCC. VM Reference DT, VM DTBO, and Debug policy");
        System.out.println("            can optionally be specified");
        System.out.println(
                "Usage: pvmfw-tool <out> <pvmfw.bin> <bcc.dat> [VM reference DT] [VM DTBO] [debug"
                        + " policy]");
    }

    public static void main(String[] args) {
        if (args.length < 3 || args.length > 6) {
            printUsage();
            System.exit(1);
        }

        File out = new File(args[0]);
        File pvmfwBin = new File(args[1]);
        File bccData = new File(args[2]);

        File vmReferenceDt = null;
        File vmDtbo = null;
        File dp = null;
        if (args.length > 3) {
            vmReferenceDt = new File(args[3]);
        }
        if (args.length > 4) {
            vmDtbo = new File(args[4]);
        }
        if (args.length > 5) {
            dp = new File(args[5]);
        }

        try {
            Pvmfw.Builder builder =
                    new Pvmfw.Builder(pvmfwBin, bccData)
                            .setVmReferenceDt(vmReferenceDt)
                            .setDebugPolicyOverlay(dp)
                            .setVmDtbo(vmDtbo);
            if (vmReferenceDt == null) {
                builder.setVersion(1, 1);
            } else {
                builder.setVersion(1, 2);
            }

            Pvmfw pvmfw = builder.build();
            pvmfw.serialize(out);
        } catch (IOException e) {
            e.printStackTrace();
            printUsage();
            System.exit(1);
        }
    }
}
