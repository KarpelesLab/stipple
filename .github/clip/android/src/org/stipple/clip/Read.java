package org.stipple.clip;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;

/** Reads the clipboard once it has focus, logs what it holds, and goes. */
public class Read extends Activity {
    private boolean read;

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus || read) {
            return;
        }
        read = true;
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData clip = clipboard.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            Log.i("stipple-clip", "read nothing");
        } else {
            CharSequence text = clip.getItemAt(0).coerceToText(this);
            Log.i("stipple-clip", "read \"" + text + "\"");
        }
        finish();
    }
}
