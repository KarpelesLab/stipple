package org.stipple.servicedemo;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * A receiver the manifest fragment declares, which the system instantiates
 * by name: `am broadcast -n org.stipple.servicedemo/.PingReceiver --es text hi`.
 *
 * Its native method is found by name in the app's library:
 * `Java_org_stipple_servicedemo_PingReceiver_ping` (examples/androidservice/lib.rs).
 */
public final class PingReceiver extends BroadcastReceiver {
    static {
        System.loadLibrary("androidservice");
    }

    private static native String ping(String text);

    @Override
    public void onReceive(Context context, Intent intent) {
        // The answer goes back to `am broadcast` ("Broadcast completed: ... data=").
        setResultData(ping(intent.getStringExtra("text")));
    }
}
