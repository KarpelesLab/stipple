package com.android.billingclient.api;
import java.util.List;
public final class QueryProductDetailsParams {
    List<Product> list;
    public static Builder newBuilder() { return new Builder(); }
    public static final class Builder {
        List<Product> list;
        public Builder setProductList(List<Product> l) { list = l; return this; }
        public QueryProductDetailsParams build() { QueryProductDetailsParams p = new QueryProductDetailsParams(); p.list = list; return p; }
    }
    public static final class Product {
        String id, type;
        public static Builder newBuilder() { return new Builder(); }
        public static final class Builder {
            String id, type;
            public Builder setProductId(String s) { id = s; return this; }
            public Builder setProductType(String s) { type = s; return this; }
            public Product build() { Product p = new Product(); p.id = id; p.type = type; return p; }
        }
    }
}
