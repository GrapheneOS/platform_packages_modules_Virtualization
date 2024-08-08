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

package com.android.virtualization.vmlauncher;

import android.crosvm.ICrosvmAndroidDisplayService;
import android.graphics.PixelFormat;
import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.system.virtualizationservice_internal.IVirtualizationServiceInternal;
import android.util.Log;
import android.view.SurfaceControl;
import android.view.SurfaceHolder;
import android.view.SurfaceView;

import libcore.io.IoBridge;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/** Presents Android-side surface where VM can use as a display */
class DisplayProvider {
    private static final String TAG = MainActivity.TAG;
    private final SurfaceView mMainView;
    private final SurfaceView mCursorView;
    private final IVirtualizationServiceInternal mVirtService;
    private CursorHandler mCursorHandler;

    DisplayProvider(SurfaceView mainView, SurfaceView cursorView) {
        mMainView = mainView;
        mCursorView = cursorView;

        mMainView.setSurfaceLifecycle(SurfaceView.SURFACE_LIFECYCLE_FOLLOWS_ATTACHMENT);
        mMainView.getHolder().addCallback(new Callback(Callback.SurfaceKind.MAIN));

        mCursorView.setSurfaceLifecycle(SurfaceView.SURFACE_LIFECYCLE_FOLLOWS_ATTACHMENT);
        mCursorView.getHolder().addCallback(new Callback(Callback.SurfaceKind.CURSOR));
        mCursorView.getHolder().setFormat(PixelFormat.RGBA_8888);
        // TODO: do we need this z-order?
        mCursorView.setZOrderMediaOverlay(true);

        IBinder b = ServiceManager.waitForService("android.system.virtualizationservice");
        mVirtService = IVirtualizationServiceInternal.Stub.asInterface(b);
        try {
            // To ensure that the previous display service is removed.
            mVirtService.clearDisplayService();
        } catch (RemoteException e) {
            throw new RuntimeException("Failed to clear prior display service", e);
        }
    }

    void notifyDisplayIsGoingToInvisible() {
        // When the display is going to be invisible (by putting in the background), save the frame
        // of the main surface so that we can re-draw it next time the display becomes visible. This
        // is to save the duration of time where nothing is drawn by VM.
        try {
            getDisplayService().saveFrameForSurface(false /* forCursor */);
        } catch (RemoteException e) {
            throw new RuntimeException("Failed to save frame for the main surface", e);
        }
    }

    private synchronized ICrosvmAndroidDisplayService getDisplayService() {
        try {
            IBinder b = mVirtService.waitDisplayService();
            return ICrosvmAndroidDisplayService.Stub.asInterface(b);
        } catch (Exception e) {
            throw new RuntimeException("Error while getting display service", e);
        }
    }

    private class Callback implements SurfaceHolder.Callback {
        enum SurfaceKind {
            MAIN,
            CURSOR
        }

        private final SurfaceKind mSurfaceKind;

        Callback(SurfaceKind kind) {
            mSurfaceKind = kind;
        }

        private boolean isForCursor() {
            return mSurfaceKind == SurfaceKind.CURSOR;
        }

        @Override
        public void surfaceCreated(SurfaceHolder holder) {
            try {
                getDisplayService().setSurface(holder.getSurface(), isForCursor());
            } catch (Exception e) {
                // TODO: don't consume this exception silently. For some unknown reason, setSurface
                // call above throws IllegalArgumentException and that fails the surface
                // configuration.
                Log.e(TAG, "Failed to present surface " + mSurfaceKind + " to VM", e);
            }

            try {
                switch (mSurfaceKind) {
                    case MAIN:
                        getDisplayService().drawSavedFrameForSurface(isForCursor());
                        break;
                    case CURSOR:
                        ParcelFileDescriptor stream = createNewCursorStream();
                        getDisplayService().setCursorStream(stream);
                        break;
                }
            } catch (Exception e) {
                // TODO: don't consume exceptions here too
                Log.e(TAG, "Failed to configure surface " + mSurfaceKind, e);
            }
        }

        @Override
        public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
            // TODO: support resizeable display. We could actually change the display size that the
            // VM sees, or keep the size and render it by fitting it in the new surface.
        }

        @Override
        public void surfaceDestroyed(SurfaceHolder holder) {
            try {
                getDisplayService().removeSurface(isForCursor());
            } catch (RemoteException e) {
                throw new RuntimeException("Error while destroying surface for " + mSurfaceKind, e);
            }
        }
    }

    private ParcelFileDescriptor createNewCursorStream() {
        if (mCursorHandler != null) {
            mCursorHandler.interrupt();
        }
        ParcelFileDescriptor[] pfds;
        try {
            pfds = ParcelFileDescriptor.createSocketPair();
        } catch (IOException e) {
            throw new RuntimeException("Failed to create socketpair for cursor stream", e);
        }
        mCursorHandler = new CursorHandler(pfds[0]);
        mCursorHandler.start();
        return pfds[1];
    }

    /**
     * Thread reading cursor coordinate from a stream, and updating the position of the cursor
     * surface accordingly.
     */
    private class CursorHandler extends Thread {
        private final ParcelFileDescriptor mStream;
        private final SurfaceControl mCursor;
        private final SurfaceControl.Transaction mTransaction;

        CursorHandler(ParcelFileDescriptor stream) {
            mStream = stream;
            mCursor = DisplayProvider.this.mCursorView.getSurfaceControl();
            mTransaction = new SurfaceControl.Transaction();

            SurfaceControl main = DisplayProvider.this.mMainView.getSurfaceControl();
            mTransaction.reparent(mCursor, main).apply();
        }

        @Override
        public void run() {
            try {
                ByteBuffer byteBuffer = ByteBuffer.allocate(8 /* (x: u32, y: u32) */);
                byteBuffer.order(ByteOrder.LITTLE_ENDIAN);
                while (true) {
                    if (Thread.interrupted()) {
                        Log.d(TAG, "CursorHandler thread interrupted!");
                        return;
                    }
                    byteBuffer.clear();
                    int bytes =
                            IoBridge.read(
                                    mStream.getFileDescriptor(),
                                    byteBuffer.array(),
                                    0,
                                    byteBuffer.array().length);
                    if (bytes == -1) {
                        Log.e(TAG, "cannot read from cursor stream, stop the handler");
                        return;
                    }
                    float x = (float) (byteBuffer.getInt() & 0xFFFFFFFF);
                    float y = (float) (byteBuffer.getInt() & 0xFFFFFFFF);
                    mTransaction.setPosition(mCursor, x, y).apply();
                }
            } catch (IOException e) {
                Log.e(TAG, "failed to run CursorHandler", e);
            }
        }
    }
}
