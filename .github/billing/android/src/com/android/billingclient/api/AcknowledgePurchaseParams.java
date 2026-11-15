package com.android.billingclient.api;
public final class AcknowledgePurchaseParams {
    String token;
    public static Builder newBuilder() { return new Builder(); }
    public static final class Builder {
        String token;
        public Builder setPurchaseToken(String s) { token = s; return this; }
        public AcknowledgePurchaseParams build() { AcknowledgePurchaseParams p = new AcknowledgePurchaseParams(); p.token = token; return p; }
    }
}
