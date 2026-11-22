package org.stipple.servicedemo;

import android.content.Context;
import androidx.startup.Initializer;
import java.util.Collections;
import java.util.List;

/**
 * An androidx.startup initializer (only in the build with libraries,
 * examples/androidservice/package-libraries.sh): compiled against the
 * library's classes.jar, which is dexed into the app with it, and run by the
 * library's InitializationProvider as the process starts, which finds it
 * through the meta-data in libraries/manifest-fragment.xml.
 *
 * androidx.startup matches that meta-data against its own string resource,
 * R.string.androidx_startup, from the AAR's res/: compiled by cargo-stipple,
 * with an R class in the library's package. Reading it here shows it.
 */
public final class PingInitializer implements Initializer<String> {
    static {
        System.loadLibrary("androidservice");
    }

    /** Found by name: `Java_org_stipple_servicedemo_PingInitializer_initialized`. */
    private static native String initialized(String key);

    @Override
    public String create(Context context) {
        return initialized(context.getString(androidx.startup.R.string.androidx_startup));
    }

    @Override
    public List<Class<? extends Initializer<?>>> dependencies() {
        return Collections.emptyList();
    }
}
