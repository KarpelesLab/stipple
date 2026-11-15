package com.android.billingclient.api;
public final class BillingResult {
    final int code; final String msg;
    BillingResult(int code, String msg) { this.code = code; this.msg = msg; }
    public int getResponseCode() { return code; }
    public String getDebugMessage() { return msg; }
}
