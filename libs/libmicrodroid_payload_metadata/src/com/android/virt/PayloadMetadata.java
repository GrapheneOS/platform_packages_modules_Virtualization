package com.android.virt;

import java.io.DataOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

/** Provides utility to create/read/write PayloadMetadata */
public class PayloadMetadata {
    public static void write(PayloadMetadataProtos.Metadata metadata, File file)
            throws IOException {
        byte[] message = metadata.toByteArray();

        try (DataOutputStream os = new DataOutputStream(new FileOutputStream(file))) {
            // write length prefix (4-byte, big-endian)
            os.writeInt(message.length);
            // write the message
            os.write(message);
        }
    }

    public static PayloadMetadataProtos.Metadata metadata(
            String configPath,
            PayloadMetadataProtos.ApkPayload apk,
            Iterable<? extends PayloadMetadataProtos.ApexPayload> apexes) {
        return PayloadMetadataProtos.Metadata.newBuilder()
                .setVersion(1)
                .setConfigPath(configPath)
                .setApk(apk)
                .addAllApexes(apexes)
                .build();
    }

    public static PayloadMetadataProtos.ApkPayload apk(String name) {
        return PayloadMetadataProtos.ApkPayload.newBuilder()
                .setName(name)
                .setPayloadPartitionName("microdroid-apk")
                .setIdsigPartitionName("microdroid-apk-idsig")
                .build();
    }

    public static PayloadMetadataProtos.ApexPayload apex(String name) {
        return PayloadMetadataProtos.ApexPayload.newBuilder()
                .setName(name)
                .setIsFactory(true)
                .setPartitionName(name)
                .build();
    }
}
