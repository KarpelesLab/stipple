package com.android.billingclient.api;
import android.app.Activity;
import android.content.Context;
import java.util.ArrayList;
import java.util.List;
public abstract class BillingClient {
    public static Builder newBuilder(Context c) { return new Builder(); }
    public abstract void startConnection(BillingClientStateListener l);
    public abstract boolean isReady();
    public abstract void queryProductDetailsAsync(QueryProductDetailsParams p, ProductDetailsResponseListener l);
    public abstract BillingResult launchBillingFlow(Activity a, BillingFlowParams p);
    public abstract void queryPurchasesAsync(QueryPurchasesParams p, PurchasesResponseListener l);
    public abstract void consumeAsync(ConsumeParams p, ConsumeResponseListener l);
    public abstract void acknowledgePurchase(AcknowledgePurchaseParams p, AcknowledgePurchaseResponseListener l);
    public static final class Builder {
        PurchasesUpdatedListener listener; boolean pending;
        public Builder setListener(PurchasesUpdatedListener l) { listener = l; return this; }
        public Builder enablePendingPurchases(PendingPurchasesParams p) { pending = true; return this; }
        public Builder enableAutoServiceReconnection() { return this; }
        public BillingClient build() { Fake.log("build pending=" + pending + " listener=" + (listener != null)); return new Impl(listener); }
    }
    static final class Impl extends BillingClient {
        final PurchasesUpdatedListener listener; int n;
        Impl(PurchasesUpdatedListener l) { listener = l; }
        public void startConnection(BillingClientStateListener l) {
            Fake.log("startConnection");
            Fake.later(() -> l.onBillingSetupFinished(Fake.ok()));
        }
        public boolean isReady() { return true; }
        public void queryProductDetailsAsync(QueryProductDetailsParams p, ProductDetailsResponseListener l) {
            List<ProductDetails> out = new ArrayList<>();
            for (QueryProductDetailsParams.Product q : p.list) {
                Fake.log("query " + q.id + " " + q.type);
                if (!q.id.startsWith("unknown")) out.add(new ProductDetails(q.id, q.type));
            }
            Fake.later(() -> l.onProductDetailsResponse(Fake.ok(), new QueryProductDetailsResult(out)));
        }
        public BillingResult launchBillingFlow(Activity a, BillingFlowParams p) {
            BillingFlowParams.ProductDetailsParams item = p.list.get(0);
            String id = item.details.getProductId();
            Fake.log("launch " + id + " offer=" + item.token + " activity=" + (a != null));
            n++;
            final Purchase bought = new Purchase(id, "token-" + id + "-" + n, "GPA.0000-" + n, 1, false);
            Fake.later(() -> {
                List<Purchase> l = new ArrayList<>();
                l.add(bought);
                listener.onPurchasesUpdated(Fake.ok(), l);
            });
            return Fake.ok();
        }
        public void queryPurchasesAsync(QueryPurchasesParams p, PurchasesResponseListener l) {
            Fake.log("queryPurchases " + p.type);
            List<Purchase> out = new ArrayList<>();
            if (p.type.equals("inapp")) {
                out.add(new Purchase("stipple_demo_interrupted", "token-interrupted", "GPA.1111", 1, false));
                out.add(new Purchase("stipple_demo_old", "token-old", "GPA.2222", 1, true));
                out.add(new Purchase("stipple_demo_pending", "token-pending", null, 2, false));
            } else {
                out.add(new Purchase("stipple_demo_vip", "token-vip", "GPA.3333", 1, true));
            }
            Fake.later(() -> l.onQueryPurchasesResponse(Fake.ok(), out));
        }
        public void consumeAsync(ConsumeParams p, ConsumeResponseListener l) {
            Fake.log("consume " + p.token);
            Fake.later(() -> l.onConsumeResponse(Fake.ok(), p.token));
        }
        public void acknowledgePurchase(AcknowledgePurchaseParams p, AcknowledgePurchaseResponseListener l) {
            Fake.log("acknowledge " + p.token);
            Fake.later(() -> l.onAcknowledgePurchaseResponse(Fake.ok()));
        }
    }
}
