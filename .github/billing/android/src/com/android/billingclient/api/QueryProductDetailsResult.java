package com.android.billingclient.api;
import java.util.List;
public final class QueryProductDetailsResult {
    final List<ProductDetails> list;
    QueryProductDetailsResult(List<ProductDetails> l) { list = l; }
    public List<ProductDetails> getProductDetailsList() { return list; }
}
