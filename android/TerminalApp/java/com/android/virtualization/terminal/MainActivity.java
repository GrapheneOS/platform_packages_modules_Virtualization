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
package com.android.virtualization.terminal;

import android.app.Activity;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.webkit.WebChromeClient;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.TextView;

import com.android.virtualization.vmlauncher.VmLauncherServices;

public class MainActivity extends Activity implements VmLauncherServices.VmLauncherServiceCallback {
    private static final String TAG = "VmTerminalApp";
    private String mVmIpAddr;
    private WebView mWebView;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        VmLauncherServices.startVmLauncherService(this, this);

        setContentView(R.layout.activity_headless);
        mWebView = (WebView) findViewById(R.id.webview);
        mWebView.getSettings().setDatabaseEnabled(true);
        mWebView.getSettings().setDomStorageEnabled(true);
        mWebView.getSettings().setJavaScriptEnabled(true);
        mWebView.setWebChromeClient(new WebChromeClient());
        mWebView.setWebViewClient(
                new WebViewClient() {
                    @Override
                    public boolean shouldOverrideUrlLoading(WebView view, String url) {
                        view.loadUrl(url);
                        return true;
                    }
                });
    }

    private void gotoURL(String url) {
        runOnUiThread(() -> mWebView.loadUrl(url));
    }

    public void onVmStart() {
        Log.i(TAG, "onVmStart()");
    }

    public void onVmStop() {
        Log.i(TAG, "onVmStop()");
        finish();
    }

    public void onVmError() {
        Log.i(TAG, "onVmError()");
        finish();
    }

    public void onIpAddrAvailable(String ipAddr) {
        mVmIpAddr = ipAddr;
        ((TextView) findViewById(R.id.ip_addr_textview)).setText(mVmIpAddr);

        // TODO(b/359523803): Use AVF API to be notified when shell is ready instead of using dealy
        new Handler(Looper.getMainLooper())
                .postDelayed(() -> gotoURL("http://" + mVmIpAddr + ":7681"), 2000);
    }
}
