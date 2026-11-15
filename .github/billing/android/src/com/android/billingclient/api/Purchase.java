package com.android.billingclient.api;
import java.util.ArrayList;
import java.util.List;
public class Purchase {
    final String product, token, order; final int state; final boolean acked;
    Purchase(String product, String token, String order, int state, boolean acked) {
        this.product = product; this.token = token; this.order = order; this.state = state; this.acked = acked;
    }
    public int getPurchaseState() { return state; }
    public List<String> getProducts() { List<String> l = new ArrayList<>(); l.add(product); return l; }
    public String getOrderId() { return order; }
    public String getPurchaseToken() { return token; }
    public long getPurchaseTime() { return 1700000000000L; }
    public String getOriginalJson() { return "{\"productId\":\"" + product + "\",\"purchaseToken\":\"" + token + "\"}"; }
    public String getSignature() { return "c2lnbmF0dXJl"; }
    public boolean isAcknowledged() { return acked; }
    public int getQuantity() { return 1; }
}
