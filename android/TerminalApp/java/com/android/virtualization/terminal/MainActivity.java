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

import android.content.Context;
import android.content.Intent;
import android.net.http.SslError;
import android.os.Bundle;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Log;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.accessibility.AccessibilityManager;
import android.webkit.ClientCertRequest;
import android.webkit.SslErrorHandler;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Toast;

import androidx.appcompat.app.AppCompatActivity;

import com.android.virtualization.vmlauncher.VmLauncherServices;

import com.google.android.material.appbar.MaterialToolbar;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.RandomAccessFile;
import java.net.InetAddress;
import java.net.MalformedURLException;
import java.net.URL;
import java.net.UnknownHostException;
import java.security.Key;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;

public class MainActivity extends AppCompatActivity
        implements VmLauncherServices.VmLauncherServiceCallback,
                AccessibilityManager.TouchExplorationStateChangeListener {

    private static final String TAG = "VmTerminalApp";
    private static final String VM_ADDR = "192.168.0.2";
    private static final int TTYD_PORT = 7681;
    private static final int REQUEST_CODE_INSTALLER = 0x33;

    private X509Certificate[] mCertificates;
    private PrivateKey mPrivateKey;
    private WebView mWebView;
    private AccessibilityManager mAccessibilityManager;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        checkForUpdate();
        try {
            // No resize for now.
            long newSizeInBytes = 0;
            diskResize(this, newSizeInBytes);
        } catch (IOException e) {
            Log.e(TAG, "Failed to resize disk", e);
            Toast.makeText(this, "Error resizing disk: " + e.getMessage(), Toast.LENGTH_LONG)
                    .show();
        }

        setContentView(R.layout.activity_headless);

        MaterialToolbar toolbar = (MaterialToolbar) findViewById(R.id.toolbar);
        setSupportActionBar(toolbar);
        mWebView = (WebView) findViewById(R.id.webview);
        mWebView.getSettings().setDatabaseEnabled(true);
        mWebView.getSettings().setDomStorageEnabled(true);
        mWebView.getSettings().setJavaScriptEnabled(true);
        mWebView.setWebChromeClient(new WebChromeClient());

        mAccessibilityManager = getSystemService(AccessibilityManager.class);
        mAccessibilityManager.addTouchExplorationStateChangeListener(this);

        connectToTerminalService();
        readClientCertificate();
    }

    private URL getTerminalServiceUrl() {
        boolean needsAccessibility = mAccessibilityManager.isTouchExplorationEnabled();
        String file = "/";
        String query = needsAccessibility ? "?screenReaderMode=true" : "";

        try {
            return new URL("https", VM_ADDR, TTYD_PORT, file + query);
        } catch (MalformedURLException e) {
            // this cannot happen
            return null;
        }
    }

    private void readClientCertificate() {
        // TODO(b/363235314): instead of using the key in asset, it should be generated in runtime
        // and then provisioned in the vm via virtio-fs
        try (InputStream keystoreFileStream =
                getClass().getResourceAsStream("/assets/client.p12")) {
            KeyStore keyStore = KeyStore.getInstance("PKCS12");
            String password = "1234";
            String alias = "1";

            keyStore.load(keystoreFileStream, password != null ? password.toCharArray() : null);
            Key key = keyStore.getKey(alias, password.toCharArray());
            if (key instanceof PrivateKey) {
                mPrivateKey = (PrivateKey) key;
                Certificate cert = keyStore.getCertificate(alias);
                mCertificates = new X509Certificate[1];
                mCertificates[0] = (X509Certificate) cert;
            }
        } catch (Exception e) {
            Log.e(TAG, e.getMessage());
        }
    }

    private void connectToTerminalService() {
        Log.i(TAG, "URL=" + getTerminalServiceUrl().toString());
        mWebView.setWebViewClient(
                new WebViewClient() {
                    @Override
                    public boolean shouldOverrideUrlLoading(
                            WebView view, WebResourceRequest request) {
                        return false;
                    }

                    @Override
                    public void onReceivedError(
                            WebView view, WebResourceRequest request, WebResourceError error) {
                        switch (error.getErrorCode()) {
                            case WebViewClient.ERROR_CONNECT:
                            case WebViewClient.ERROR_HOST_LOOKUP:
                                view.reload();
                                return;
                            default:
                                String url = request.getUrl().toString();
                                CharSequence msg = error.getDescription();
                                Log.e(TAG, "Failed to load " + url + ": " + msg);
                        }
                    }

                    @Override
                    public void onPageFinished(WebView view, String url) {
                        URL loadedUrl = null;
                        try {
                            loadedUrl = new URL(url);
                        } catch (MalformedURLException e) {
                            // cannot happen.
                        }
                        Log.i(TAG, "on page finished. URL=" + loadedUrl);
                        if (getTerminalServiceUrl().toString().equals(url)) {
                            android.os.Trace.endAsyncSection("executeTerminal", 0);
                            view.setVisibility(View.VISIBLE);
                        }
                    }

                    @Override
                    public void onReceivedClientCertRequest(
                            WebView view, ClientCertRequest request) {
                        if (mPrivateKey != null && mCertificates != null) {
                            request.proceed(mPrivateKey, mCertificates);
                            return;
                        }
                        super.onReceivedClientCertRequest(view, request);
                    }

                    @Override
                    public void onReceivedSslError(
                            WebView view, SslErrorHandler handler, SslError error) {
                        // ttyd uses self-signed certificate
                        handler.proceed();
                    }
                });
        new Thread(
                        () -> {
                            waitUntilVmStarts();
                            runOnUiThread(
                                    () -> mWebView.loadUrl(getTerminalServiceUrl().toString()));
                        })
                .start();
    }

    private void diskResize(Context context, long sizeInBytes) throws IOException {
        try {
            if (sizeInBytes == 0) {
                return;
            }
            File file = getPartitionFile(context, "root_part");
            String filePath = file.getAbsolutePath();
            Log.d(TAG, "Disk-resize in progress for partition: " + filePath);

            long currentSize = Os.stat(filePath).st_size;
            runE2fsck(filePath);
            if (sizeInBytes > currentSize) {
                allocateSpace(file, sizeInBytes);
            }

            resizeFilesystem(filePath, sizeInBytes);
        } catch (ErrnoException e) {
            Log.e(TAG, "ErrnoException during disk resize", e);
            throw new IOException("ErrnoException during disk resize", e);
        } catch (IOException e) {
            Log.e(TAG, "Failed to resize disk", e);
            throw e;
        }
    }

    private static File getPartitionFile(Context context, String fileName)
            throws FileNotFoundException {
        File file = new File(context.getFilesDir(), fileName);
        if (!file.exists()) {
            Log.d(TAG, fileName + " - file not found");
            throw new FileNotFoundException("File not found: " + fileName);
        }
        return file;
    }

    private static void allocateSpace(File file, long sizeInBytes) throws IOException {
        try {
            RandomAccessFile raf = new RandomAccessFile(file, "rw");
            FileDescriptor fd = raf.getFD();
            Os.posix_fallocate(fd, 0, sizeInBytes);
            raf.close();
            Log.d(TAG, "Allocated space to: " + sizeInBytes + " bytes");
        } catch (ErrnoException e) {
            Log.e(TAG, "Failed to allocate space", e);
            throw new IOException("Failed to allocate space", e);
        }
    }

    private static void runE2fsck(String filePath) throws IOException {
        try {
            runCommand("/system/bin/e2fsck", "-f", filePath);
            Log.d(TAG, "e2fsck completed: " + filePath);
        } catch (IOException e) {
            Log.e(TAG, "Failed to run e2fsck", e);
            throw e;
        }
    }

    private static void resizeFilesystem(String filePath, long sizeInBytes) throws IOException {
        long sizeInMB = sizeInBytes / (1024 * 1024);
        if (sizeInMB == 0) {
            Log.e(TAG, "Invalid size: " + sizeInBytes + " bytes");
            throw new IllegalArgumentException("Size cannot be zero MB");
        }
        String sizeArg = sizeInMB + "M";
        try {
            runCommand("/system/bin/resize2fs", filePath, sizeArg);
            Log.d(TAG, "resize2fs completed: " + filePath + ", size: " + sizeArg);
        } catch (IOException e) {
            Log.e(TAG, "Failed to run resize2fs", e);
            throw e;
        }
    }

    private static void runCommand(String... command) throws IOException {
        try {
            Process process = new ProcessBuilder(command).redirectErrorStream(true).start();
            process.waitFor();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException("Command interrupted", e);
        }
    }

    private static void waitUntilVmStarts() {
        InetAddress addr = null;
        try {
            addr = InetAddress.getByName(VM_ADDR);
        } catch (UnknownHostException e) {
            // this can never happen.
        }
        try {
            while (!addr.isReachable(10000)) {}
        } catch (IOException e) {
            // give up on network error
            throw new RuntimeException(e);
        }
        return;
    }

    @Override
    protected void onDestroy() {
        getSystemService(AccessibilityManager.class).removeTouchExplorationStateChangeListener(this);
        VmLauncherServices.stopVmLauncherService(this);
        super.onDestroy();
    }

    @Override
    public void onVmStart() {
        Log.i(TAG, "onVmStart()");
    }

    @Override
    public void onVmStop() {
        Toast.makeText(this, R.string.vm_stop_message, Toast.LENGTH_SHORT).show();
        Log.i(TAG, "onVmStop()");
        finish();
    }

    @Override
    public void onVmError() {
        Toast.makeText(this, R.string.vm_error_message, Toast.LENGTH_SHORT).show();
        Log.i(TAG, "onVmError()");
        finish();
    }

    @Override
    public void onIpAddrAvailable(String ipAddr) {
        // TODO: remove this
    }

    @Override
    public boolean onCreateOptionsMenu(Menu menu) {
        getMenuInflater().inflate(R.menu.main_menu, menu);
        return true;
    }

    @Override
    public boolean onOptionsItemSelected(MenuItem item) {
        int id = item.getItemId();
        if (id == R.id.menu_item_settings) {
            Intent intent = new Intent(this, SettingsActivity.class);
            this.startActivity(intent);
            return true;
        }
        return super.onOptionsItemSelected(item);
    }

    @Override
    public void onTouchExplorationStateChanged(boolean enabled) {
        connectToTerminalService();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);

        if (requestCode == REQUEST_CODE_INSTALLER) {
            if (resultCode != RESULT_OK) {
                Log.e(TAG, "Failed to start VM. Installer returned error.");
                finish();
            }
            startVm();
        }
    }

    private void checkForUpdate() {
        Intent intent = new Intent(this, InstallerActivity.class);
        startActivityForResult(intent, REQUEST_CODE_INSTALLER);
    }

    private void startVm() {
        android.os.Trace.beginAsyncSection("executeTerminal", 0);
        VmLauncherServices.startVmLauncherService(this, this);
    }
}
