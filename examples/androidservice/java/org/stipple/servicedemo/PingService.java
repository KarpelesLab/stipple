package org.stipple.servicedemo;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

/**
 * A started service the manifest fragment declares:
 * `am startservice -n org.stipple.servicedemo/.PingService --es text hi`.
 * Android may start the app's process for it alone, with no activity.
 *
 * Its native method is bound by `RegisterNatives` in the library's
 * `JNI_OnLoad` (examples/androidservice/lib.rs), which runs when the static
 * initializer below loads the library.
 */
public final class PingService extends Service {
    static {
        System.loadLibrary("androidservice");
    }

    private static native void started(String text, int startId);

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        started(intent != null ? intent.getStringExtra("text") : null, startId);
        stopSelf(startId);
        return START_NOT_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
