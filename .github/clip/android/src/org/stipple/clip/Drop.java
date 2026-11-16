package org.stipple.clip;

import android.app.Activity;
import android.content.ClipData;
import android.database.Cursor;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.util.Log;
import android.view.DragEvent;
import android.view.Gravity;
import android.view.View;
import android.widget.TextView;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;

/**
 * Another app's drop target, for examples/androiddemo/test-drag-apps.sh: a
 * window that takes any drag (in split screen or a freeform window beside
 * androiddemo) and logs what it dropped: a text, or a URI's display name and
 * bytes, read through the ContentResolver with the drag's URI permissions.
 */
public class Drop extends Activity {
    private static final String TAG = "stipple-clip";

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        TextView view = new TextView(this);
        view.setText("Drop here");
        view.setTextSize(32);
        view.setGravity(Gravity.CENTER);
        view.setBackgroundColor(Color.rgb(0xdd, 0xee, 0xff));
        view.setOnDragListener(new View.OnDragListener() {
            @Override
            public boolean onDrag(View v, DragEvent event) {
                switch (event.getAction()) {
                    case DragEvent.ACTION_DRAG_STARTED:
                        Log.i(TAG, "drag started: " + event.getClipDescription());
                        return true;
                    case DragEvent.ACTION_DRAG_ENTERED:
                        Log.i(TAG, "drag entered");
                        return true;
                    case DragEvent.ACTION_DROP:
                        dropped(event);
                        return true;
                    default:
                        return true;
                }
            }
        });
        setContentView(view);
        Log.i(TAG, "drop target shown");
    }

    private void dropped(DragEvent event) {
        requestDragAndDropPermissions(event);
        ClipData clip = event.getClipData();
        if (clip == null) {
            Log.i(TAG, "drop without data");
            return;
        }
        for (int i = 0; i < clip.getItemCount(); i++) {
            ClipData.Item item = clip.getItemAt(i);
            if (item.getText() != null) {
                Log.i(TAG, "drop text \"" + item.getText() + "\"");
                continue;
            }
            Uri uri = item.getUri();
            if (uri == null) {
                Log.i(TAG, "drop item " + i + " has neither text nor a URI");
                continue;
            }
            try {
                String name = "?";
                Cursor c = getContentResolver().query(uri, null, null, null, null);
                if (c != null) {
                    int col = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                    if (c.moveToFirst() && col >= 0) {
                        name = c.getString(col);
                    }
                    c.close();
                }
                InputStream in = getContentResolver().openInputStream(uri);
                ByteArrayOutputStream out = new ByteArrayOutputStream();
                byte[] buf = new byte[4096];
                int n;
                while ((n = in.read(buf)) > 0) {
                    out.write(buf, 0, n);
                }
                in.close();
                Log.i(TAG, "drop file \"" + name + "\" type " + getContentResolver().getType(uri)
                        + " (" + out.size() + " bytes): \"" + out.toString("UTF-8") + "\"");
            } catch (Exception e) {
                Log.i(TAG, "drop of " + uri + " failed: " + e);
            }
        }
    }
}
