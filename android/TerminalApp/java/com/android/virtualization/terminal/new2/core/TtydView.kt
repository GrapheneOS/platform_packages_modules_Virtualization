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
package com.android.virtualization.terminal.new2.core

import android.content.Context
import android.graphics.fonts.FontStyle
import android.net.http.SslError
import android.util.AttributeSet
import android.util.Log
import android.view.ViewGroup
import android.view.accessibility.AccessibilityManager
import android.webkit.ClientCertRequest
import android.webkit.ConsoleMessage
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import com.android.virtualization.terminal.CertificateUtils
import com.android.virtualization.terminal.TerminalView
import java.net.MalformedURLException
import java.net.URL
import java.security.cert.X509Certificate

class TtydView @JvmOverloads constructor(context: Context, attrs: AttributeSet? = null) :
    TerminalView(context, attrs) {

    var onTerminalReady: (() -> Unit)? = null
    var onTerminalDisconnected: (() -> Unit)? = null

    init {
        settings.domStorageEnabled = true
        settings.javaScriptEnabled = true
        settings.cacheMode = WebSettings.LOAD_DEFAULT

        addJavascriptInterface(
            object {
                @android.webkit.JavascriptInterface
                fun onTerminalReady() {
                    this@TtydView.onTerminalReady!!.invoke()
                }

                @android.webkit.JavascriptInterface
                fun onTerminalDisconnected() {
                    post { this@TtydView.onTerminalDisconnected?.invoke() }
                }
            },
            "android",
        )
        webViewClient = TtydWebViewClient()
        layoutParams =
            ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
        isFocusable = true
        isFocusableInTouchMode = true

        enableJavascriptConsoleDebug()
    }

    fun load(ipAddress: String, port: Int) {
        val url = getTerminalServiceUrl(ipAddress, port)
        Log.d("TtydView", "Loading URL: ${url.toString()}")
        loadUrl(url.toString())
    }

    private fun getTerminalServiceUrl(ipAddress: String?, port: Int): URL? {
        val config = resources.configuration
        val a11yManager = context.getSystemService(AccessibilityManager::class.java)
        // TODO: Always enable screenReaderMode (b/395845063)
        val query =
            ("?fontSize=" +
                (config.fontScale * 13).toInt() +
                "&fontWeight=" +
                (FontStyle.FONT_WEIGHT_NORMAL + config.fontWeightAdjustment) +
                "&fontWeightBold=" +
                (FontStyle.FONT_WEIGHT_BOLD + config.fontWeightAdjustment) +
                "&screenReaderMode=" +
                a11yManager.isEnabled +
                "&disableResizeOverlay=true")

        try {
            return URL("https", ipAddress, port, query)
        } catch (e: MalformedURLException) {
            // this cannot happen
            return null
        }
    }

    override fun onCheckIsTextEditor(): Boolean {
        return true
    }

    private fun enableJavascriptConsoleDebug() {
        webChromeClient =
            object : WebChromeClient() {
                override fun onConsoleMessage(msg: ConsoleMessage?): Boolean {
                    Log.d("TTYD", "${msg?.message()}")
                    return true
                }
            }
    }

    private inner class TtydWebViewClient : WebViewClient() {
        override fun onReceivedClientCertRequest(view: WebView, request: ClientCertRequest) {
            val pke = CertificateUtils.createOrGetKey()
            val certificates = arrayOf<X509Certificate>(pke.certificate as X509Certificate)
            request.proceed(pke.privateKey, certificates)
        }

        override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: SslError?) {
            // ttyd uses self-signed certificate
            handler.proceed()
        }

        override fun onReceivedError(
            view: WebView,
            request: WebResourceRequest,
            error: WebResourceError,
        ) {
            Log.e("TtydWebViewClient", "WebView Error: ${error.errorCode} - ${error.description}")
            // Consider errors like network loss, host lookup failure as disconnection
            if (
                error.errorCode == WebViewClient.ERROR_HOST_LOOKUP ||
                    error.errorCode == WebViewClient.ERROR_CONNECT ||
                    error.errorCode == WebViewClient.ERROR_TIMEOUT ||
                    error.errorCode == WebViewClient.ERROR_BAD_URL
            ) {
                this@TtydView.onTerminalDisconnected?.invoke()
            }
            super.onReceivedError(view, request, error)
        }

        override fun onPageFinished(view: WebView, url: String) {
            super.onPageFinished(view, url)
            // TODO: explain reason for this
            val js =
                """
                (function() {
                    var notifyReady = function() {
                        window.term.focus();
                        window.android.onTerminalReady();
                    };
                    var check = function() {
                        var xterm = document.querySelector('.terminal.xterm');
                        if (window.term && xterm) {
                            console.log("xterm found");
                            setTimeout(notifyReady, 500);
                        } else {
                            console.log("xterm not found. waiting...");
                            setTimeout(check, 100);
                        }
                    };
                    check();
                })();
            """
            view.evaluateJavascript(js, null)
        }
    }
}
