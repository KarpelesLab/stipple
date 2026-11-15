package com.android.billingclient.api;
import java.util.List;
public final class BillingFlowParams {
    List<ProductDetailsParams> list;
    public static Builder newBuilder() { return new Builder(); }
    public static final class Builder {
        List<ProductDetailsParams> list;
        public Builder setProductDetailsParamsList(List<ProductDetailsParams> l) { list = l; return this; }
        public BillingFlowParams build() { BillingFlowParams p = new BillingFlowParams(); p.list = list; return p; }
    }
    public static final class ProductDetailsParams {
        ProductDetails details; String token;
        public static Builder newBuilder() { return new Builder(); }
        public static final class Builder {
            ProductDetails details; String token;
            public Builder setProductDetails(ProductDetails d) { details = d; return this; }
            public Builder setOfferToken(String t) { token = t; return this; }
            public ProductDetailsParams build() { ProductDetailsParams p = new ProductDetailsParams(); p.details = details; p.token = token; return p; }
        }
    }
}
