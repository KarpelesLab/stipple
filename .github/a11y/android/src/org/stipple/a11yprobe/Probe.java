// An accessibility service standing in for TalkBack on CI's emulator (whose
// images don't ship TalkBack): enabled through
// `settings put secure enabled_accessibility_services`, it runs in its own
// process and reaches the Stipple app only through the system's accessibility
// framework, as TalkBack does. Once connected it reads the active window's
// node tree, moves accessibility focus through the nodes TalkBack would stop
// on, in its reading order (a swipe right at a time), then clicks a button
// (TalkBack's double tap), sets the text field's text, and scrolls the list.
// Everything it sees goes to logcat under the `a11yprobe` tag, for
// .github/a11y/android/test-probe.sh to check.
package org.stipple.a11yprobe;

import android.accessibilityservice.AccessibilityService;
import android.graphics.Rect;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;
import java.util.ArrayList;
import java.util.List;

public class Probe extends AccessibilityService {
    static final String TAG = "a11yprobe";
    static final String TARGET = "com.karpeleslab.stipple";

    final Handler handler = new Handler(Looper.getMainLooper());
    int attempts;
    int nodes;
    List<AccessibilityNodeInfo> order;
    int step;
    int moved;

    static void log(String line) {
        Log.i(TAG, "probe " + line);
    }

    @Override
    protected void onServiceConnected() {
        log("connected");
        later(2000, new Runnable() {
            public void run() {
                start();
            }
        });
    }

    void later(long ms, Runnable r) {
        handler.postDelayed(r, ms);
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent e) {
        if (e.getPackageName() == null || !TARGET.contentEquals(e.getPackageName())) {
            return;
        }
        int type = e.getEventType();
        if (type == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED) {
            return; // every frame that changes the tree: too many to log
        }
        log("event " + AccessibilityEvent.eventTypeToString(type) + " text=" + e.getText()
                + (e.getContentDescription() != null ? " desc=" + e.getContentDescription() : ""));
    }

    @Override
    public void onInterrupt() {}

    static String label(AccessibilityNodeInfo n) {
        CharSequence t = n.getText();
        if (t != null && t.length() > 0) {
            return t.toString();
        }
        CharSequence d = n.getContentDescription();
        return d != null ? d.toString() : "";
    }

    static String describe(AccessibilityNodeInfo n) {
        Rect r = new Rect();
        n.getBoundsInScreen(r);
        StringBuilder actions = new StringBuilder();
        for (AccessibilityNodeInfo.AccessibilityAction a : n.getActionList()) {
            if (actions.length() > 0) {
                actions.append(',');
            }
            actions.append(a.getLabel() != null ? a.getLabel().toString() : actionName(a.getId()));
        }
        return "class=" + n.getClassName() + " text=\"" + label(n) + "\" bounds=" + r.toShortString()
                + " visible=" + n.isVisibleToUser() + " clickable=" + n.isClickable()
                + " focusable=" + n.isFocusable() + " editable=" + n.isEditable()
                + " actions=[" + actions + "]";
    }

    static String actionName(int id) {
        switch (id) {
            case AccessibilityNodeInfo.ACTION_FOCUS: return "focus";
            case AccessibilityNodeInfo.ACTION_CLEAR_FOCUS: return "clear_focus";
            case AccessibilityNodeInfo.ACTION_SELECT: return "select";
            case AccessibilityNodeInfo.ACTION_CLICK: return "click";
            case AccessibilityNodeInfo.ACTION_LONG_CLICK: return "long_click";
            case AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS: return "a11y_focus";
            case AccessibilityNodeInfo.ACTION_CLEAR_ACCESSIBILITY_FOCUS: return "clear_a11y_focus";
            case AccessibilityNodeInfo.ACTION_SCROLL_FORWARD: return "scroll_forward";
            case AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD: return "scroll_backward";
            case AccessibilityNodeInfo.ACTION_SET_TEXT: return "set_text";
            default: return "0x" + Integer.toHexString(id);
        }
    }

    /// Where TalkBack would stop: a visible node that acts or has something
    /// to say (not a bare container).
    static boolean speaks(AccessibilityNodeInfo n) {
        return n.isVisibleToUser()
                && (n.isClickable() || n.isEditable() || label(n).trim().length() > 0);
    }

    void walk(AccessibilityNodeInfo n, int depth, List<AccessibilityNodeInfo> order) {
        nodes++;
        StringBuilder indent = new StringBuilder();
        for (int i = 0; i < depth; i++) {
            indent.append("  ");
        }
        log("node " + indent + describe(n));
        if (depth > 0 && speaks(n)) {
            order.add(n);
        }
        for (int i = 0; i < n.getChildCount(); i++) {
            AccessibilityNodeInfo c = n.getChild(i);
            if (c != null) {
                walk(c, depth + 1, order);
            }
        }
    }

    AccessibilityNodeInfo find(AccessibilityNodeInfo n, String text, boolean editable) {
        if (editable ? n.isEditable() : text.equals(label(n))) {
            return n;
        }
        for (int i = 0; i < n.getChildCount(); i++) {
            AccessibilityNodeInfo c = n.getChild(i);
            AccessibilityNodeInfo found = c == null ? null : find(c, text, editable);
            if (found != null) {
                return found;
            }
        }
        return null;
    }

    AccessibilityNodeInfo findScrollable(AccessibilityNodeInfo n) {
        if (n.getActionList().contains(AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_FORWARD)) {
            return n;
        }
        for (int i = 0; i < n.getChildCount(); i++) {
            AccessibilityNodeInfo c = n.getChild(i);
            AccessibilityNodeInfo found = c == null ? null : findScrollable(c);
            if (found != null) {
                return found;
            }
        }
        return null;
    }

    AccessibilityNodeInfo targetRoot() {
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null || root.getPackageName() == null || !TARGET.contentEquals(root.getPackageName())) {
            return null;
        }
        return root;
    }

    void start() {
        final AccessibilityNodeInfo root = targetRoot();
        if (root == null) {
            if (++attempts < 20) {
                log("waiting for the app's window");
                later(1000, new Runnable() {
                    public void run() {
                        start();
                    }
                });
            } else {
                log("no window of " + TARGET);
                log("done");
            }
            return;
        }
        // 1. The tree.
        List<AccessibilityNodeInfo> order = new ArrayList<AccessibilityNodeInfo>();
        nodes = 0;
        walk(root, 0, order);
        log("tree nodes=" + nodes + " stops=" + order.size());

        // 2. Linear navigation: accessibility focus on each stop in turn (a
        // swipe right each), then, once the framework has heard of it, the
        // window's accessibility focus must be on that node.
        this.order = order;
        step = 0;
        moved = 0;
        later(300, new Runnable() {
            public void run() {
                focusNext();
            }
        });
    }

    void focusNext() {
        if (step > 0) {
            AccessibilityNodeInfo want = order.get(step - 1);
            AccessibilityNodeInfo fresh = targetRoot();
            AccessibilityNodeInfo f = fresh == null ? null
                    : fresh.findFocus(AccessibilityNodeInfo.FOCUS_ACCESSIBILITY);
            // The same node: its class and place (a label may change, as the
            // seconds counter does).
            Rect a = new Rect();
            Rect b = new Rect();
            want.getBoundsInScreen(a);
            if (f != null) {
                f.getBoundsInScreen(b);
            }
            boolean same = f != null && a.equals(b) && f.getClassName() != null
                    && f.getClassName().equals(want.getClassName());
            if (same) {
                moved++;
            }
            log("focused \"" + label(want) + "\" -> " + (f == null ? "none" : "\"" + label(f) + "\""));
        }
        if (step < order.size()) {
            AccessibilityNodeInfo n = order.get(step++);
            boolean done = n.performAction(AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS);
            log("focus \"" + label(n) + "\" -> " + done);
            later(300, new Runnable() {
                public void run() {
                    focusNext();
                }
            });
            return;
        }
        log("linear navigation " + moved + "/" + order.size());
        act();
    }

    void act() {
        AccessibilityNodeInfo root = targetRoot();
        if (root == null) {
            log("the app's window went away");
            log("done");
            return;
        }
        // 3. Activation (TalkBack's double tap).
        AccessibilityNodeInfo tap = find(root, "Tap me", false);
        log("click \"Tap me\" -> " + (tap != null && tap.performAction(AccessibilityNodeInfo.ACTION_CLICK)));

        // 4. Text: replace the field's text, as a braille display or voice
        // input does through TalkBack.
        AccessibilityNodeInfo field = find(root, "", true);
        Bundle args = new Bundle();
        args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "TalkBack");
        log("set text -> " + (field != null && field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)));

        // 5. Scrolling (TalkBack's two-finger swipe, or its scroll action).
        AccessibilityNodeInfo list = findScrollable(root);
        if (list == null) {
            log("scroll forward: no node offers it");
        } else {
            log("scroll forward " + describe(list) + " -> "
                    + list.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD));
        }

        later(1500, new Runnable() {
            public void run() {
                AccessibilityNodeInfo again = targetRoot();
                AccessibilityNodeInfo f = again == null ? null : find(again, "", true);
                log("field now \"" + (f == null ? "?" : label(f)) + "\"");
                log("done");
            }
        });
    }
}
