package com.android.billingclient.api;
public interface BillingClientStateListener { void onBillingSetupFinished(BillingResult r); void onBillingServiceDisconnected(); }
