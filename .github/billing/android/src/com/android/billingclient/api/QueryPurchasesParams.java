package com.android.billingclient.api;
public final class QueryPurchasesParams {
    String type;
    public static Builder newBuilder() { return new Builder(); }
    public static final class Builder {
        String type;
        public Builder setProductType(String s) { type = s; return this; }
        public QueryPurchasesParams build() { QueryPurchasesParams p = new QueryPurchasesParams(); p.type = type; return p; }
    }
}
