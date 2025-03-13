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
import android.graphics.drawable.Icon
import android.graphics.fonts.FontStyle
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.ConditionVariable
import android.os.Environment
import android.os.SystemProperties
import android.provider.Settings
import android.util.DisplayMetrics
import android.util.Log
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.view.accessibility.AccessibilityManager
import android.widget.Button
import android.widget.HorizontalScrollView
import android.widget.RelativeLayout
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultCallback
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts.StartActivityForResult
import androidx.lifecycle.ViewModelProvider
import androidx.viewpager2.widget.ViewPager2
import com.android.internal.annotations.VisibleForTesting
import com.android.microdroid.test.common.DeviceProperties
import com.android.system.virtualmachine.flags.Flags.terminalGuiSupport
import com.android.virtualization.terminal.ErrorActivity.Companion.start
import com.android.virtualization.terminal.InstalledImage.Companion.getDefault
import com.android.virtualization.terminal.VmLauncherService.Companion.run
import com.android.virtualization.terminal.VmLauncherService.Companion.stop
import com.android.virtualization.terminal.VmLauncherService.VmLauncherServiceCallback
import com.google.android.material.tabs.TabLayout
import com.google.android.material.tabs.TabLayoutMediator
import java.io.IOException
import java.net.MalformedURLException
import java.net.URL
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

public class MainActivity :
    BaseActivity(),
    VmLauncherServiceCallback,
    AccessibilityManager.AccessibilityStateChangeListener {
    var displayMenu: Button? = null
    var tabAddButton: Button? = null
    val bootCompleted = ConditionVariable()
    lateinit var modifierKeysController: ModifierKeysController
    private lateinit var tabScrollView: HorizontalScrollView
    private lateinit var executorService: ExecutorService
    private lateinit var image: InstalledImage
    private lateinit var accessibilityManager: AccessibilityManager
    private lateinit var manageExternalStorageActivityResultLauncher: ActivityResultLauncher<Intent>
    private lateinit var terminalViewModel: TerminalViewModel
    private lateinit var viewPager: ViewPager2
    private lateinit var tabLayout: TabLayout
    private lateinit var terminalTabAdapter: TerminalTabAdapter
    private val terminalInfo = CompletableFuture<TerminalInfo>()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lockOrientationIfNecessary()

        image = getDefault(this)

        val launchInstaller = installIfNecessary()

        initializeUi()

        accessibilityManager =
            getSystemService<AccessibilityManager>(AccessibilityManager::class.java)
        accessibilityManager.addAccessibilityStateChangeListener(this)

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

    private fun initializeUi() {
        terminalViewModel = ViewModelProvider(this)[TerminalViewModel::class.java]
        setContentView(R.layout.activity_headless)
        tabLayout = findViewById<TabLayout>(R.id.tab_layout)
        displayMenu = findViewById<Button>(R.id.display_button)
        tabAddButton = findViewById<Button>(R.id.tab_add_button)
        tabScrollView = findViewById<HorizontalScrollView>(R.id.tab_scrollview)
        val modifierKeysContainerView =
            findViewById<RelativeLayout>(R.id.modifier_keys_container) as ViewGroup

        findViewById<Button>(R.id.settings_button).setOnClickListener {
            val intent = Intent(this, SettingsActivity::class.java)
            this.startActivity(intent)
        }

        displayMenu?.also {
            it.visibility = if (terminalGuiSupport()) View.VISIBLE else View.GONE
            it.setEnabled(false)
            if (terminalGuiSupport()) {
                it.setOnClickListener {
                    val intent = Intent(this, DisplayActivity::class.java)
                    intent.flags =
                        intent.flags or
                            Intent.FLAG_ACTIVITY_NEW_TASK or
                            Intent.FLAG_ACTIVITY_CLEAR_TASK
                    this.startActivity(intent)
                }
            }
        }

        modifierKeysController = ModifierKeysController(this, modifierKeysContainerView)

        terminalTabAdapter = TerminalTabAdapter(this)
        viewPager = findViewById(R.id.pager)
        viewPager.adapter = terminalTabAdapter
        viewPager.isUserInputEnabled = false
        viewPager.offscreenPageLimit = 2

        TabLayoutMediator(tabLayout, viewPager, false, false) { _: TabLayout.Tab?, _: Int -> }
            .attach()

        addTerminalTab()

        tabAddButton?.setOnClickListener { addTerminalTab() }
    }

    private fun addTerminalTab() {
        val tab = tabLayout.newTab()
        tab.setCustomView(R.layout.tabitem_terminal)
        viewPager.offscreenPageLimit += 1
        terminalTabAdapter.addTab()
        tab.customView!!
            .findViewById<Button>(R.id.tab_close_button)
            .setOnClickListener(
                View.OnClickListener { _: View? ->
                    if (terminalTabAdapter.tabs.size == 1) {
                        finishAndRemoveTask()
                    }
                    viewPager.offscreenPageLimit -= 1
                    terminalTabAdapter.deleteTab(tab.position)
                    tabLayout.removeTab(tab)
                }
            )
        // Add and select the tab
        tabLayout.addTab(tab, true)
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
        // TODO: Always enable screenReaderMode (b/395845063)
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

    fun connectToTerminalService(terminalView: TerminalView) {
        terminalInfo.thenAcceptAsync(
            { info ->
                val url = getTerminalServiceUrl(info.ipAddress, info.port)
                runOnUiThread({ terminalView.loadUrl(url.toString()) })
            },
            executorService,
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

    override fun onTerminalAvailable(info: TerminalInfo) {
        terminalInfo.complete(info)
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

    override fun onAccessibilityStateChanged(enabled: Boolean) {
        terminalViewModel.terminalViews.forEach { terminalView ->
            connectToTerminalService(terminalView)
        }
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
        stopIntent.setAction(VmLauncherService.ACTION_SHUTDOWN_VM)
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

        run(this, this, notification, getDisplayInfo())
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
