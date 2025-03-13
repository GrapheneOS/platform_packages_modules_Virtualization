/*
 * Copyright (C) 2025 The Android Open Source Project
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

import android.content.Intent
import android.graphics.Bitmap
import android.net.http.SslError
import android.os.Bundle
import android.util.Log
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.webkit.ClientCertRequest
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.fragment.app.Fragment
import androidx.lifecycle.ViewModelProvider
import com.android.system.virtualmachine.flags.Flags.terminalGuiSupport
import com.android.virtualization.terminal.CertificateUtils.createOrGetKey
import com.android.virtualization.terminal.CertificateUtils.writeCertificateToFile
import java.security.PrivateKey
import java.security.cert.X509Certificate

class TerminalTabFragment() : Fragment() {
    private lateinit var terminalView: TerminalView
    private lateinit var bootProgressView: View
    private lateinit var id: String
    private var certificates: Array<X509Certificate>? = null
    private var privateKey: PrivateKey? = null
    private lateinit var terminalViewModel: TerminalViewModel

    override fun onCreateView(
        inflater: LayoutInflater,
        container: ViewGroup?,
        savedInstanceState: Bundle?,
    ): View {
        val view = inflater.inflate(R.layout.fragment_terminal_tab, container, false)
        arguments?.let { id = it.getString("id")!! }
        return view
    }

    override fun onViewCreated(view: View, savedInstanceState: Bundle?) {
        super.onViewCreated(view, savedInstanceState)
        terminalViewModel = ViewModelProvider(this)[TerminalViewModel::class.java]
        terminalView = view.findViewById(R.id.webview)
        bootProgressView = view.findViewById(R.id.boot_progress)
        initializeWebView()
        readClientCertificate()

        terminalView.webViewClient = TerminalWebViewClient()

        if (savedInstanceState != null) {
            terminalView.restoreState(savedInstanceState)
        } else {
            (activity as MainActivity).connectToTerminalService(terminalView)
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        terminalView.saveState(outState)
    }

    private fun initializeWebView() {
        terminalView.settings.databaseEnabled = true
        terminalView.settings.domStorageEnabled = true
        terminalView.settings.javaScriptEnabled = true
        terminalView.settings.cacheMode = WebSettings.LOAD_DEFAULT

        terminalView.webChromeClient = WebChromeClient()
        terminalView.webViewClient = TerminalWebViewClient()

        (activity as MainActivity).modifierKeysController.addTerminalView(terminalView)
        terminalViewModel.terminalViews.add(terminalView)
    }

    private inner class TerminalWebViewClient : WebViewClient() {
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
                    Log.e(MainActivity.TAG, "Failed to load $url: $msg")
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
                            bootProgressView.visibility = View.GONE
                            terminalView.visibility = View.VISIBLE
                            terminalView.mapTouchToMouseEvent()
                            updateMainActivity()
                        }
                    }
                },
            )
        }

        override fun onReceivedClientCertRequest(view: WebView?, request: ClientCertRequest) {
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

    private fun updateMainActivity() {
        val mainActivity = (activity as MainActivity)
        if (terminalGuiSupport()) {
            mainActivity.displayMenu!!.visibility = View.VISIBLE
            mainActivity.displayMenu!!.isEnabled = true
        }
        mainActivity.tabAddButton!!.isEnabled = true
        mainActivity.bootCompleted.open()
    }

    private fun readClientCertificate() {
        val pke = createOrGetKey()
        writeCertificateToFile(activity!!, pke.certificate)
        privateKey = pke.privateKey
        certificates = arrayOf<X509Certificate>(pke.certificate as X509Certificate)
    }

    companion object {
        const val TAG: String = "VmTerminalApp"
    }

    override fun onDestroy() {
        terminalViewModel.terminalViews.remove(terminalView)
        super.onDestroy()
    }
}
