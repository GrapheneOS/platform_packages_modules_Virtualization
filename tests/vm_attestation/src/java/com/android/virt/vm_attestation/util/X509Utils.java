/**
 * Copyright (C) 2024 The Android Open Source Project
 *
 * <p>Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file
 * except in compliance with the License. You may obtain a copy of the License at
 *
 * <p>http://www.apache.org/licenses/LICENSE-2.0
 *
 * <p>Unless required by applicable law or agreed to in writing, software distributed under the
 * License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either
 * express or implied. See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.android.virt.vm_attestation.util;

import static com.google.common.truth.Truth.assertThat;
import static com.google.common.truth.Truth.assertWithMessage;

import org.bouncycastle.asn1.ASN1Boolean;
import org.bouncycastle.asn1.ASN1Encodable;
import org.bouncycastle.asn1.ASN1OctetString;
import org.bouncycastle.asn1.ASN1Sequence;
import org.bouncycastle.asn1.DEROctetString;
import org.bouncycastle.asn1.DERUTF8String;

import java.io.ByteArrayInputStream;
import java.security.Signature;
import java.security.cert.CertPathValidator;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.cert.PKIXParameters;
import java.security.cert.TrustAnchor;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Set;

/**
 * Provides utility methods for parsing and verifying X.509 certificate chain issued from pVM remote
 * attestation.
 */
public class X509Utils {
    private static final String AVF_ATTESTATION_EXTENSION_OID = "1.3.6.1.4.1.11129.2.1.29.1";

    /** Validates and parses the given DER-encoded X.509 certificate chain. */
    public static X509Certificate[] validateAndParseX509CertChain(byte[] x509CertChain)
            throws Exception {
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        ByteArrayInputStream in = new ByteArrayInputStream(x509CertChain);
        ArrayList<Certificate> certs = new ArrayList<>(factory.generateCertificates(in));
        X509Certificate[] certChain = certs.toArray(new X509Certificate[0]);
        validateCertChain(certChain);
        return certChain;
    }

    private static void validateCertChain(X509Certificate[] certChain) throws Exception {
        X509Certificate rootCert = certChain[certChain.length - 1];
        // The root certificate should be self-signed.
        rootCert.verify(rootCert.getPublicKey());

        // Only add the self-signed root certificate as trust anchor.
        // All the other certificates in the chain should be signed by the previous cert's key.
        Set<TrustAnchor> trustAnchors =
                Set.of(new TrustAnchor(rootCert, /* nameConstraints= */ null));

        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        CertPathValidator validator = CertPathValidator.getInstance("PKIX");
        PKIXParameters parameters = new PKIXParameters(trustAnchors);
        parameters.setRevocationEnabled(false);
        validator.validate(factory.generateCertPath(Arrays.asList(certChain)), parameters);
    }

    /**
     * Verifies the AVF related certificates in the given certificate chain. The AVF Attestation
     * extension should be found in the leaf certificate.
     */
    public static void verifyAvfRelatedCerts(
            X509Certificate[] certChain, byte[] challenge, String payloadApk) throws Exception {
        assertThat(certChain.length).isGreaterThan(2);
        assertWithMessage("The first certificate should be generated in the RKP VM")
                .that(certChain[0].getSubjectX500Principal().getName())
                .isEqualTo("CN=Android Protected Virtual Machine Key");
        verifyAvfAttestationExtension(certChain[0], challenge, payloadApk);

        assertWithMessage("The second certificate should contain AVF in the subject")
                .that(certChain[1].getSubjectX500Principal().getName())
                .contains("O=AVF");
    }

    private static void verifyAvfAttestationExtension(
            X509Certificate cert, byte[] challenge, String payloadApk) throws Exception {
        byte[] extensionValue = cert.getExtensionValue(AVF_ATTESTATION_EXTENSION_OID);
        ASN1OctetString extString = ASN1OctetString.getInstance(extensionValue);
        ASN1Sequence seq = ASN1Sequence.getInstance(extString.getOctets());
        // AVF attestation extension should contain 3 elements in the following format:
        //
        //  AttestationExtension ::= SEQUENCE {
        //     attestationChallenge       OCTET_STRING,
        //     isVmSecure                 BOOLEAN,
        //     vmComponents               SEQUENCE OF VmComponent,
        //  }
        //   VmComponent ::= SEQUENCE {
        //     name               UTF8String,
        //     securityVersion    INTEGER,
        //     codeHash           OCTET STRING,
        //     authorityHash      OCTET STRING,
        //  }
        assertThat(seq).hasSize(3);

        ASN1OctetString expectedChallenge = new DEROctetString(challenge);
        assertThat(seq.getObjectAt(0)).isEqualTo(expectedChallenge);
        assertWithMessage("The VM should be unsecure as it is debuggable.")
                .that(seq.getObjectAt(1))
                .isEqualTo(ASN1Boolean.FALSE);
        ASN1Sequence vmComponents = ASN1Sequence.getInstance(seq.getObjectAt(2));
        assertExtensionContainsPayloadApk(vmComponents, payloadApk);
    }

    private static void assertExtensionContainsPayloadApk(
            ASN1Sequence vmComponents, String payloadApk) throws Exception {
        DERUTF8String payloadApkName = new DERUTF8String("apk:" + payloadApk);
        boolean found = false;
        for (ASN1Encodable encodable : vmComponents) {
            ASN1Sequence vmComponent = ASN1Sequence.getInstance(encodable);
            assertThat(vmComponent).hasSize(4);
            if (payloadApkName.equals(vmComponent.getObjectAt(0))) {
                assertWithMessage("Payload APK should not be found twice.").that(found).isFalse();
                found = true;
            }
        }
        assertWithMessage("vmComponents should contain the payload APK.").that(found).isTrue();
    }

    /** Verifies the given signature using the public key from the given certificate. */
    public static void verifySignature(
            X509Certificate publicKeyCert, byte[] messageToSign, byte[] signature)
            throws Exception {
        Signature sig = Signature.getInstance("SHA256withECDSA");
        sig.initVerify(publicKeyCert.getPublicKey());
        sig.update(messageToSign);
        assertThat(sig.verify(signature)).isTrue();
    }
}
