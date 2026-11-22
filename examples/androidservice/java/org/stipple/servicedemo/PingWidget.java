package org.stipple.servicedemo;

import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.Context;
import android.content.Intent;
import android.view.View;
import android.widget.RemoteViews;
import android.widget.TextView;

/**
 * An app widget provider. Its layout (res/layout/ping_widget.xml), label
 * (res/values/strings.xml) and provider info (res/xml/ping_widget_info.xml,
 * which the manifest fragment's meta-data names) are the app's own
 * resources, which cargo-stipple compiles with aapt2 (android.res); the
 * R class aapt2 writes is compiled with these sources.
 *
 * A home screen updates it with APPWIDGET_UPDATE, which only the system
 * may send; `am broadcast -a org.stipple.servicedemo.UPDATE_WIDGET
 * -n org.stipple.servicedemo/.PingWidget` runs onUpdate too (for tests).
 */
public final class PingWidget extends AppWidgetProvider {
    static {
        System.loadLibrary("androidservice");
    }

    /** Found by name: `Java_org_stipple_servicedemo_PingWidget_update`. */
    private static native String update(String label, String text);

    @Override
    public void onReceive(Context context, Intent intent) {
        if ("org.stipple.servicedemo.UPDATE_WIDGET".equals(intent.getAction())) {
            onUpdate(context, AppWidgetManager.getInstance(context), new int[0]);
        } else {
            super.onReceive(context, intent);
        }
    }

    @Override
    public void onUpdate(Context context, AppWidgetManager manager, int[] ids) {
        String label = context.getString(R.string.ping_widget_label);
        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.ping_widget);
        // Inflated here too, to read the layout's text back.
        View view = views.apply(context, null);
        TextView text = (TextView) view.findViewById(R.id.ping_widget_text);
        views.setTextViewText(R.id.ping_widget_text, update(label, text.getText().toString()));
        try {
            manager.updateAppWidget(ids, views);
        } catch (RuntimeException e) {
            // An id no host placed (adb's broadcast): nothing to update.
        }
    }
}
