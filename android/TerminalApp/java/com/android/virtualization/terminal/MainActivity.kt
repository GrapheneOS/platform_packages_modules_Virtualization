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
package com.android.virtualization.terminal

import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.drawable.Icon
import android.graphics.fonts.FontStyle
import android.net.Uri
import android.net.http.SslError
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Bundle
import android.os.ConditionVariable
import android.os.Environment
import android.os.SystemProperties
import android.os.Trace
import android.provider.Settings
import android.util.DisplayMetrics
import android.util.Log
import android.view.KeyEvent
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.view.accessibility.AccessibilityManager
import android.webkit.ClientCertRequest
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultCallback
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts.StartActivityForResult
import com.android.internal.annotations.VisibleForTesting
import com.android.microdroid.test.common.DeviceProperties
import com.android.system.virtualmachine.flags.Flags.terminalGuiSupport
import com.android.virtualization.terminal.CertificateUtils.createOrGetKey
import com.android.virtualization.terminal.CertificateUtils.writeCertificateToFile
import com.android.virtualization.terminal.ErrorActivity.Companion.start
import com.android.virtualization.terminal.InstalledImage.Companion.getDefault
import com.android.virtualization.terminal.VmLauncherService.Companion.run
import com.android.virtualization.terminal.VmLauncherService.Companion.stop
import com.android.virtualization.terminal.VmLauncherService.VmLauncherServiceCallback
import com.google.android.material.appbar.MaterialToolbar
import java.io.IOException
import java.lang.Exception
import java.net.MalformedURLException
import java.net.URL
import java.security.PrivateKey
import java.security.cert.X509Certificate
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

public class MainActivity :
    BaseActivity(),
    VmLauncherServiceCallback,
    AccessibilityManager.AccessibilityStateChangeListener {
    private lateinit var executorService: ExecutorService
    private lateinit var image: InstalledImage
    private var certificates: Array<X509Certificate>? = null
    private var privateKey: PrivateKey? = null
    private lateinit var terminalContainer: ViewGroup
    private lateinit var terminalView: TerminalView
    private lateinit var accessibilityManager: AccessibilityManager
    private val bootCompleted = ConditionVariable()
    private lateinit var manageExternalStorageActivityResultLauncher: ActivityResultLauncher<Intent>
    private lateinit var modifierKeysController: ModifierKeysController
    private var displayMenu: MenuItem? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lockOrientationIfNecessary()

        image = getDefault(this)

        val launchInstaller = installIfNecessary()

        setContentView(R.layout.activity_headless)

        val toolbar = findViewById<MaterialToolbar>(R.id.toolbar)
        setSupportActionBar(toolbar)
        terminalView = findViewById<TerminalView>(R.id.webview)
        terminalView.getSettings().setDomStorageEnabled(true)
        terminalView.getSettings().setJavaScriptEnabled(true)
        terminalView.getSettings().setCacheMode(WebSettings.LOAD_NO_CACHE)
        terminalView.setWebChromeClient(WebChromeClient())

        terminalContainer = terminalView.parent as ViewGroup

        modifierKeysController = ModifierKeysController(this, terminalView, terminalContainer)

        accessibilityManager =
            getSystemService<AccessibilityManager>(AccessibilityManager::class.java)
        accessibilityManager.addAccessibilityStateChangeListener(this)

        readClientCertificate()

        manageExternalStorageActivityResultLauncher =
            registerForActivityResult<Intent, ActivityResult>(
                StartActivityForResult(),
                ActivityResultCallback { startVm() },
            )
        executorService =
            Executors.newSingleThreadExecutor(TerminalThreadFactory(applicationContext))

        // if installer is launched, it will be handled in onActivityResult
        if (!launchInstaller) {
            if (!Environment.isExternalStorageManager()) {
                requestStoragePermissions(this, manageExternalStorageActivityResultLauncher)
            } else {
                startVm()
            }
        }
    }

    private fun lockOrientationIfNecessary() {
        val hasHwQwertyKeyboard = resources.configuration.keyboard == Configuration.KEYBOARD_QWERTY
        if (hasHwQwertyKeyboard) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED)
        } else if (resources.getBoolean(R.bool.terminal_portrait_only)) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_PORTRAIT)
        }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        lockOrientationIfNecessary()
        modifierKeysController.update()
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (Build.isDebuggable() && event.keyCode == KeyEvent.KEYCODE_UNKNOWN) {
            if (event.action == KeyEvent.ACTION_UP) {
                start(this, Exception("Debug: KeyEvent.KEYCODE_UNKNOWN"))
            }
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    private fun requestStoragePermissions(
        context: Context,
        activityResultLauncher: ActivityResultLauncher<Intent>,
    ) {
        val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION)
        val uri = Uri.fromParts("package", context.getPackageName(), null)
        intent.setData(uri)
        activityResultLauncher.launch(intent)
    }

    private fun getTerminalServiceUrl(ipAddress: String?, port: Int): URL? {
        val config = resources.configuration

        val query =
            ("?fontSize=" +
                (config.fontScale * FONT_SIZE_DEFAULT).toInt() +
                "&fontWeight=" +
                (FontStyle.FONT_WEIGHT_NORMAL + config.fontWeightAdjustment) +
                "&fontWeightBold=" +
                (FontStyle.FONT_WEIGHT_BOLD + config.fontWeightAdjustment) +
                "&screenReaderMode=" +
                accessibilityManager.isEnabled +
                "&titleFixed=" +
                getString(R.string.app_name))

        try {
            return URL("https", ipAddress, port, "/$query")
        } catch (e: MalformedURLException) {
            // this cannot happen
            return null
        }
    }

    private fun readClientCertificate() {
        val pke = createOrGetKey()
        writeCertificateToFile(this, pke.certificate)
        privateKey = pke.privateKey
        certificates = arrayOf<X509Certificate>(pke.certificate as X509Certificate)
    }

    private fun connectToTerminalService() {
        terminalView.setWebViewClient(
            object : WebViewClient() {
                private var loadFailed = false
                private var requestId: Long = 0

                override fun shouldOverrideUrlLoading(
                    view: WebView?,
                    request: WebResourceRequest?,
                ): Boolean {
                    val intent = Intent(Intent.ACTION_VIEW, request?.url)
                    intent.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    startActivity(intent)
                    return true
                }

                override fun onPageStarted(view: WebView?, url: String?, favicon: Bitmap?) {
                    loadFailed = false
                }

                override fun onReceivedError(
                    view: WebView,
                    request: WebResourceRequest,
                    error: WebResourceError,
                ) {
                    loadFailed = true
                    when (error.getErrorCode()) {
                        ERROR_CONNECT,
                        ERROR_HOST_LOOKUP,
                        ERROR_FAILED_SSL_HANDSHAKE,
                        ERROR_TIMEOUT -> {
                            view.reload()
                            return
                        }

                        else -> {
                            val url: String? = request.getUrl().toString()
                            val msg = error.getDescription()
                            Log.e(TAG, "Failed to load $url: $msg")
                        }
                    }
                }

                override fun onPageFinished(view: WebView, url: String?) {
                    if (loadFailed) {
                        return
                    }

                    requestId++
                    view.postVisualStateCallback(
                        requestId,
                        object : WebView.VisualStateCallback() {
                            override fun onComplete(completedRequestId: Long) {
                                if (completedRequestId == requestId) {
                                    Trace.endAsyncSection("executeTerminal", 0)
                                    findViewById<View?>(R.id.boot_progress).visibility = View.GONE
                                    terminalContainer.visibility = View.VISIBLE
                                    if (terminalGuiSupport()) {
                                        displayMenu?.setVisible(true)
                                        displayMenu?.setEnabled(true)
                                    }
                                    bootCompleted.open()
                                    modifierKeysController.update()
                                    terminalView.mapTouchToMouseEvent()
                                }
                            }
                        },
                    )
                }

                override fun onReceivedClientCertRequest(
                    view: WebView?,
                    request: ClientCertRequest,
                ) {
                    if (privateKey != null && certificates != null) {
                        request.proceed(privateKey, certificates)
                        return
                    }
                    super.onReceivedClientCertRequest(view, request)
                }

                override fun onReceivedSslError(
                    view: WebView?,
                    handler: SslErrorHandler,
                    error: SslError?,
                ) {
                    // ttyd uses self-signed certificate
                    handler.proceed()
                }
            }
        )

        // TODO: refactor this block as a method
        val nsdManager = getSystemService<NsdManager>(NsdManager::class.java)
        val info = NsdServiceInfo()
        info.serviceType = "_http._tcp"
        info.serviceName = "ttyd"
        nsdManager.registerServiceInfoCallback(
            info,
            executorService,
            object : NsdManager.ServiceInfoCallback {
                var loaded: Boolean = false

                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {}

                override fun onServiceInfoCallbackUnregistered() {}

                override fun onServiceLost() {}

                override fun onServiceUpdated(info: NsdServiceInfo) {
                    Log.i(TAG, "Service found: $info")
                    val ipAddress = info.hostAddresses[0].hostAddress
                    val port = info.port
                    val url = getTerminalServiceUrl(ipAddress, port)
                    if (!loaded) {
                        loaded = true
                        nsdManager.unregisterServiceInfoCallback(this)
                        runOnUiThread(Runnable { terminalView.loadUrl(url.toString()) })
                    }
                }
            },
        )
    }

    override fun onDestroy() {
        executorService.shutdown()
        getSystemService<AccessibilityManager>(AccessibilityManager::class.java)
            .removeAccessibilityStateChangeListener(this)
        stop(this)
        super.onDestroy()
    }

    override fun onVmStart() {
        Log.i(TAG, "onVmStart()")
    }

    override fun onVmStop() {
        Log.i(TAG, "onVmStop()")
        finish()
    }

    override fun onVmError() {
        Log.i(TAG, "onVmError()")
        // TODO: error cause is too simple.
        start(this, Exception("onVmError"))
    }

    override fun onCreateOptionsMenu(menu: Menu?): Boolean {
        menuInflater.inflate(R.menu.main_menu, menu)
        displayMenu =
            menu?.findItem(R.id.menu_item_display).also {
                it?.setVisible(terminalGuiSupport())
                it?.setEnabled(false)
            }
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        val id = item.getItemId()
        if (id == R.id.menu_item_settings) {
            val intent = Intent(this, SettingsActivity::class.java)
            this.startActivity(intent)
            return true
        } else if (id == R.id.menu_item_display) {
            val intent = Intent(this, DisplayActivity::class.java)
            intent.flags =
                intent.flags or Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK
            this.startActivity(intent)
            return true
        }
        return super.onOptionsItemSelected(item)
    }

    override fun onAccessibilityStateChanged(enabled: Boolean) {
        connectToTerminalService()
    }

    private val installerLauncher =
        registerForActivityResult(StartActivityForResult()) { result ->
            val resultCode = result.resultCode
            if (resultCode != RESULT_OK) {
                Log.e(TAG, "Failed to start VM. Installer returned error.")
                finish()
            }
            if (!Environment.isExternalStorageManager()) {
                requestStoragePermissions(this, manageExternalStorageActivityResultLauncher)
            } else {
                startVm()
            }
        }

    private fun installIfNecessary(): Boolean {
        // If payload from external storage exists(only for debuggable build) or there is no
        // installed image, launch installer activity.
        if (!image.isInstalled()) {
            val intent = Intent(this, InstallerActivity::class.java)
            installerLauncher.launch(intent)
            return true
        }
        return false
    }

    private fun startVm() {
        val image = getDefault(this)
        if (!image.isInstalled()) {
            return
        }

        resizeDiskIfNecessary(image)

        val tapIntent = Intent(this, MainActivity::class.java)
        tapIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val tapPendingIntent =
            PendingIntent.getActivity(this, 0, tapIntent, PendingIntent.FLAG_IMMUTABLE)

        val settingsIntent = Intent(this, SettingsActivity::class.java)
        settingsIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val settingsPendingIntent =
            PendingIntent.getActivity(this, 0, settingsIntent, PendingIntent.FLAG_IMMUTABLE)

        val stopIntent = Intent()
        stopIntent.setClass(this, VmLauncherService::class.java)
        stopIntent.setAction(VmLauncherService.ACTION_STOP_VM_LAUNCHER_SERVICE)
        val stopPendingIntent =
            PendingIntent.getService(
                this,
                0,
                stopIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val icon = Icon.createWithResource(resources, R.drawable.ic_launcher_foreground)
        val notification: Notification =
            Notification.Builder(this, Application.CHANNEL_LONG_RUNNING_ID)
                .setSilent(true)
                .setSmallIcon(R.drawable.ic_launcher_foreground)
                .setContentTitle(resources.getString(R.string.service_notification_title))
                .setContentText(resources.getString(R.string.service_notification_content))
                .setContentIntent(tapPendingIntent)
                .setOngoing(true)
                .addAction(
                    Notification.Action.Builder(
                            icon,
                            resources.getString(R.string.service_notification_settings),
                            settingsPendingIntent,
                        )
                        .build()
                )
                .addAction(
                    Notification.Action.Builder(
                            icon,
                            resources.getString(R.string.service_notification_quit_action),
                            stopPendingIntent,
                        )
                        .build()
                )
                .build()

        Trace.beginAsyncSection("executeTerminal", 0)
        run(this, this, notification, getDisplayInfo())
        connectToTerminalService()
    }

    @VisibleForTesting
    public fun waitForBootCompleted(timeoutMillis: Long): Boolean {
        return bootCompleted.block(timeoutMillis)
    }

    private fun resizeDiskIfNecessary(image: InstalledImage) {
        try {
            // TODO(b/382190982): Show snackbar message instead when it's recoverable.
            image.resize(intent.getLongExtra(KEY_DISK_SIZE, image.getSize()))
        } catch (e: IOException) {
            start(this, Exception("Failed to resize disk", e))
            return
        }
    }

    companion object {
        const val TAG: String = "VmTerminalApp"
        const val KEY_DISK_SIZE: String = "disk_size"
        private val TERMINAL_CONNECTION_TIMEOUT_MS: Int
        private const val REQUEST_CODE_INSTALLER = 0x33
        private const val FONT_SIZE_DEFAULT = 13

        init {
            val prop =
                DeviceProperties.create(
                    DeviceProperties.PropertyGetter { key: String -> SystemProperties.get(key) }
                )
            TERMINAL_CONNECTION_TIMEOUT_MS =
                if (prop.isCuttlefish() || prop.isGoldfish()) {
                    180000 // 3 minutes
                } else {
                    20000 // 20 sec
                }
        }

        private val BTN_KEY_CODE_MAP =
            mapOf(
                R.id.btn_tab to KeyEvent.KEYCODE_TAB, // Alt key sends ESC keycode
                R.id.btn_alt to KeyEvent.KEYCODE_ESCAPE,
                R.id.btn_esc to KeyEvent.KEYCODE_ESCAPE,
                R.id.btn_left to KeyEvent.KEYCODE_DPAD_LEFT,
                R.id.btn_right to KeyEvent.KEYCODE_DPAD_RIGHT,
                R.id.btn_up to KeyEvent.KEYCODE_DPAD_UP,
                R.id.btn_down to KeyEvent.KEYCODE_DPAD_DOWN,
                R.id.btn_home to KeyEvent.KEYCODE_MOVE_HOME,
                R.id.btn_end to KeyEvent.KEYCODE_MOVE_END,
                R.id.btn_pgup to KeyEvent.KEYCODE_PAGE_UP,
                R.id.btn_pgdn to KeyEvent.KEYCODE_PAGE_DOWN,
            )
    }

    fun getDisplayInfo(): DisplayInfo {
        val wm = getSystemService<WindowManager>(WindowManager::class.java)
        val metrics = wm.currentWindowMetrics
        val dispBounds = metrics.bounds

        // For now, display activity runs as landscape mode
        val height = Math.min(dispBounds.right, dispBounds.bottom)
        val width = Math.max(dispBounds.right, dispBounds.bottom)
        var dpi = (DisplayMetrics.DENSITY_DEFAULT * metrics.density).toInt()
        var refreshRate = display.refreshRate.toInt()

        return DisplayInfo(width, height, dpi, refreshRate)
    }
}
