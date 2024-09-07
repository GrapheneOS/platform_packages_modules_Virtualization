/*
 * Copyright (C) 2024 The Android Open Source Project
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

package com.android.virtualization.vmlauncher;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Parcel;
import android.os.ResultReceiver;
import android.util.Log;

import java.util.List;

public class VmLauncherServices {
    private static final String TAG = "VmLauncherServices";

    private static final String ACTION_START_VM_LAUNCHER_SERVICE =
            "android.virtualization.START_VM_LAUNCHER_SERVICE";

    private static final int RESULT_START = 0;
    private static final int RESULT_STOP = 1;
    private static final int RESULT_ERROR = 2;
    private static final int RESULT_IPADDR = 3;
    private static final String KEY_VM_IP_ADDR = "ip_addr";

    private static Intent buildVmLauncherServiceIntent(Context context) {
        Intent i = new Intent();
        i.setAction(ACTION_START_VM_LAUNCHER_SERVICE);

        Intent intent = new Intent(ACTION_START_VM_LAUNCHER_SERVICE);
        PackageManager pm = context.getPackageManager();
        List<ResolveInfo> resolveInfos =
                pm.queryIntentServices(intent, PackageManager.MATCH_DEFAULT_ONLY);
        if (resolveInfos == null || resolveInfos.size() != 1) {
            Log.e(TAG, "cannot find a service to handle ACTION_START_VM_LAUNCHER_SERVICE");
            return null;
        }
        String packageName = resolveInfos.get(0).serviceInfo.packageName;

        i.setPackage(packageName);
        return i;
    }

    public static void stopVmLauncherService(Context context) {
        Intent i = buildVmLauncherServiceIntent(context);
        context.stopService(i);
    }

    public static void startVmLauncherService(Context context, VmLauncherServiceCallback callback) {
        Intent i = buildVmLauncherServiceIntent(context);
        if (i == null) {
            return;
        }
        ResultReceiver resultReceiver =
                new ResultReceiver(new Handler(Looper.myLooper())) {
                    @Override
                    protected void onReceiveResult(int resultCode, Bundle resultData) {
                        if (callback == null) {
                            return;
                        }
                        switch (resultCode) {
                            case RESULT_START:
                                callback.onVmStart();
                                return;
                            case RESULT_STOP:
                                callback.onVmStop();
                                return;
                            case RESULT_ERROR:
                                callback.onVmError();
                                return;
                            case RESULT_IPADDR:
                                callback.onIpAddrAvailable(resultData.getString(KEY_VM_IP_ADDR));
                                return;
                        }
                    }
                };
        i.putExtra(Intent.EXTRA_RESULT_RECEIVER, getResultReceiverForIntent(resultReceiver));
        context.startForegroundService(i);
    }

    public interface VmLauncherServiceCallback {
        void onVmStart();

        void onVmStop();

        void onVmError();

        void onIpAddrAvailable(String ipAddr);
    }

    private static ResultReceiver getResultReceiverForIntent(ResultReceiver r) {
        Parcel parcel = Parcel.obtain();
        r.writeToParcel(parcel, 0);
        parcel.setDataPosition(0);
        r = ResultReceiver.CREATOR.createFromParcel(parcel);
        parcel.recycle();
        return r;
    }
}
