// A stand-in for the Google Play Billing Library, for CI: the classes and
// public signatures Stipple's JNI driver (src/platform/backend/android/
// billing.rs) calls, as javap prints them for billing 8.0.0, answering from
// memory instead of the Play Store. An emulator has no Play Store signed in
// (and the app no Play Console listing), so the real library only ever
// answers BILLING_UNAVAILABLE there; this one connects, lists products, runs
// a purchase and hands back an unfinished one, so the whole path from Rust
// through the proxies and back runs. See test-billing.sh.
package com.android.billingclient.api;

import android.util.Log;

final class Fake {
    static void log(String s) { Log.i("fakebilling", s); }

    /** Answer on another thread a moment later, as the library does. */
    static void later(Runnable r) {
        Thread t = new Thread(() -> {
            try { Thread.sleep(50); } catch (InterruptedException e) {}
            r.run();
        });
        t.start();
    }

    static BillingResult ok() { return new BillingResult(0, ""); }
}
