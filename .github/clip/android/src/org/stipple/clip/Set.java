package org.stipple.clip;

import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

/** Puts the broadcast's "text" extra on the clipboard, as another app copying. */
public class Set extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        String text = intent.getStringExtra("text");
        ClipboardManager clipboard =
                (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        try {
            clipboard.setPrimaryClip(ClipData.newPlainText("stipple-clip", text));
            Log.i("stipple-clip", "set \"" + text + "\"");
        } catch (RuntimeException e) {
            Log.i("stipple-clip", "set failed: " + e);
        }
    }
}
