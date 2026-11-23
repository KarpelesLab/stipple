// A second accessibility service in the probe's APK, for
// examples/androiddemo/test-on-device.sh's edit menu check: it clicks an item
// of a popup the app's window doesn't hold, such as the floating text edit
// menu (an ActionMode's toolbar, in a window of its own that
// `uiautomator dump` doesn't read: it reads the active window only).
//
// It asks for the interactive windows (flagRetrieveInteractiveWindows), and
// on `am broadcast -a org.stipple.a11yprobe.CLICK --es text Copy` looks
// through every window's node tree (getWindows()) for a visible node whose
// text or description is that text, and clicks it (or the nearest clickable
// node holding it). What it finds and does goes to logcat under the
// `a11yprobe` tag, as "menu ..." lines. It does nothing else: enabled on its
// own, it leaves the app alone (Probe walks the app's tree as soon as it
// connects).
package org.stipple.a11yprobe;

import android.accessibilityservice.AccessibilityService;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.graphics.Rect;
import android.os.Build;
import android.util.Log;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;
import java.util.List;

public class Menu extends AccessibilityService {
    static final String ACTION = "org.stipple.a11yprobe.CLICK";
    /** Context.RECEIVER_EXPORTED (API 33): the shell's broadcast reaches it. */
    static final int RECEIVER_EXPORTED = 0x2;

    BroadcastReceiver receiver;

    static void log(String line) {
        Log.i(Probe.TAG, "menu " + line);
    }

    @Override
    protected void onServiceConnected() {
        receiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                String text = intent.getStringExtra("text");
                if (text != null) {
                    click(text);
                }
            }
        };
        IntentFilter filter = new IntentFilter(ACTION);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(receiver, filter, RECEIVER_EXPORTED);
        } else {
            registerReceiver(receiver, filter);
        }
        log("connected");
    }

    @Override
    public void onDestroy() {
        if (receiver != null) {
            unregisterReceiver(receiver);
        }
        super.onDestroy();
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent e) {}

    @Override
    public void onInterrupt() {}

    void click(String text) {
        List<AccessibilityWindowInfo> windows = getWindows();
        log("looking for \"" + text + "\" in " + windows.size() + " window(s)");
        for (AccessibilityWindowInfo w : windows) {
            AccessibilityNodeInfo root = w.getRoot();
            Rect r = new Rect();
            w.getBoundsInScreen(r);
            log("window type=" + w.getType() + " layer=" + w.getLayer() + " bounds=" + r.toShortString()
                    + " package=" + (root == null ? "?" : root.getPackageName()));
            AccessibilityNodeInfo found = root == null ? null : find(root, text);
            if (found == null) {
                continue;
            }
            AccessibilityNodeInfo target = found;
            while (target != null && !target.isClickable()) {
                target = target.getParent();
            }
            if (target == null) {
                target = found;
            }
            Rect b = new Rect();
            target.getBoundsInScreen(b);
            boolean done = target.performAction(AccessibilityNodeInfo.ACTION_CLICK);
            log("click \"" + text + "\" (" + target.getClassName() + " " + b.toShortString()
                    + ", window type=" + w.getType() + ") -> " + done);
            return;
        }
        log("no \"" + text + "\" in any window");
    }

    static AccessibilityNodeInfo find(AccessibilityNodeInfo n, String text) {
        if (n.isVisibleToUser() && text.equals(Probe.label(n))) {
            return n;
        }
        for (int i = 0; i < n.getChildCount(); i++) {
            AccessibilityNodeInfo c = n.getChild(i);
            AccessibilityNodeInfo found = c == null ? null : find(c, text);
            if (found != null) {
                return found;
            }
        }
        return null;
    }
}
