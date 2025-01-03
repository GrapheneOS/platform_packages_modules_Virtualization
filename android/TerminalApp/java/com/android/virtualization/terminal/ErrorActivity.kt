/*
 * Copyright 2024 The Android Open Source Project
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

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.TextView
import java.io.IOException
import java.io.PrintWriter
import java.io.StringWriter
import java.lang.Exception
import java.lang.RuntimeException

class ErrorActivity : BaseActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        setContentView(R.layout.activity_error)

        val button = findViewById<View>(R.id.recovery)
        button.setOnClickListener(View.OnClickListener { _ -> launchRecoveryActivity() })
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
    }

    override fun onResume() {
        super.onResume()

        val intent = getIntent()
        val e = intent.getParcelableExtra<Exception?>(EXTRA_CAUSE, Exception::class.java)
        val cause = findViewById<TextView>(R.id.cause)
        cause.text = e?.let { getString(R.string.error_code, getStackTrace(it)) }
    }

    private fun launchRecoveryActivity() {
        val intent = Intent(this, SettingsRecoveryActivity::class.java)
        startActivity(intent)
    }

    companion object {
        private const val EXTRA_CAUSE = "cause"

        fun start(context: Context, e: Exception) {
            val intent = Intent(context, ErrorActivity::class.java)
            intent.putExtra(EXTRA_CAUSE, e)
            intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK or Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
        }

        private fun getStackTrace(e: Exception): String? {
            try {
                StringWriter().use { sWriter ->
                    PrintWriter(sWriter).use { pWriter ->
                        e.printStackTrace(pWriter)
                        return sWriter.toString()
                    }
                }
            } catch (ex: IOException) {
                // This shall never happen
                throw RuntimeException(ex)
            }
        }
    }
}
