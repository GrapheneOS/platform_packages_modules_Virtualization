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

package com.android.virtualization.terminal;

import static com.android.virtualization.terminal.MainActivity.TAG;

import android.content.Context;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.security.InvalidAlgorithmParameterException;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.NoSuchAlgorithmException;
import java.security.NoSuchProviderException;
import java.security.cert.Certificate;
import java.security.cert.CertificateEncodingException;
import java.security.cert.CertificateExpiredException;
import java.security.cert.CertificateNotYetValidException;
import java.security.cert.X509Certificate;

public class CertificateUtils {
    private static final String ALIAS = "ttyd";

    public static KeyStore.PrivateKeyEntry createOrGetKey() {
        try {
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);

            if (!ks.containsAlias(ALIAS)) {
                Log.d(TAG, "there is no keypair, will generate it");
                createKey();
            } else if (!(ks.getCertificate(ALIAS) instanceof X509Certificate)) {
                Log.d(TAG, "certificate isn't X509Certificate or it is invalid");
                createKey();
            } else {
                try {
                    ((X509Certificate) ks.getCertificate(ALIAS)).checkValidity();
                } catch (CertificateExpiredException | CertificateNotYetValidException e) {
                    Log.d(TAG, "certificate is invalid", e);
                    createKey();
                }
            }
            return ((KeyStore.PrivateKeyEntry) ks.getEntry(ALIAS, null));
        } catch (Exception e) {
            throw new RuntimeException("cannot generate or get key", e);
        }
    }

    private static void createKey()
            throws NoSuchAlgorithmException,
                    NoSuchProviderException,
                    InvalidAlgorithmParameterException {
        KeyPairGenerator kpg =
                KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore");
        kpg.initialize(
                new KeyGenParameterSpec.Builder(
                                ALIAS, KeyProperties.PURPOSE_SIGN | KeyProperties.PURPOSE_VERIFY)
                        .setDigests(KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA512)
                        .build());

        kpg.generateKeyPair();
    }

    public static void writeCertificateToFile(Context context, Certificate cert) {
        String certFileName = "ca.crt";
        File certFile = new File(context.getFilesDir(), certFileName);
        try (FileOutputStream writer = new FileOutputStream(certFile)) {
            String cert_begin = "-----BEGIN CERTIFICATE-----\n";
            String end_cert = "-----END CERTIFICATE-----\n";
            String output =
                    cert_begin
                            + Base64.encodeToString(cert.getEncoded(), Base64.DEFAULT)
                                    .replaceAll("(.{64})", "$1\n")
                            + end_cert;
            writer.write(output.getBytes());
        } catch (IOException | CertificateEncodingException e) {
            throw new RuntimeException("cannot write certs", e);
        }
    }
}
