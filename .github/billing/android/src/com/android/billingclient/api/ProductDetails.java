package com.android.billingclient.api;
import java.util.ArrayList;
import java.util.List;
public final class ProductDetails {
    String id, type;
    ProductDetails(String id, String type) { this.id = id; this.type = type; }
    public String getProductId() { return id; }
    public String getProductType() { return type; }
    public String getName() { return "Name of " + id + " 💎"; }
    public String getTitle() { return getName() + " (Stipple)"; }
    public String getDescription() { return "About " + id; }
    public OneTimePurchaseOfferDetails getOneTimePurchaseOfferDetails() {
        return type.equals("inapp") ? new OneTimePurchaseOfferDetails() : null;
    }
    public List<SubscriptionOfferDetails> getSubscriptionOfferDetails() {
        if (!type.equals("subs")) return null;
        List<SubscriptionOfferDetails> l = new ArrayList<>();
        l.add(new SubscriptionOfferDetails("intro", "token-intro"));
        l.add(new SubscriptionOfferDetails(null, "token-base"));
        return l;
    }
    public static final class OneTimePurchaseOfferDetails {
        public String getFormattedPrice() { return "¥120"; }
        public long getPriceAmountMicros() { return 120000000L; }
        public String getPriceCurrencyCode() { return "JPY"; }
    }
    public static final class SubscriptionOfferDetails {
        final String offerId, token;
        SubscriptionOfferDetails(String o, String t) { offerId = o; token = t; }
        public String getOfferId() { return offerId; }
        public String getOfferToken() { return token; }
        public PricingPhases getPricingPhases() { return new PricingPhases(offerId == null); }
    }
    public static final class PricingPhases {
        final boolean base;
        PricingPhases(boolean b) { base = b; }
        public List<PricingPhase> getPricingPhaseList() {
            List<PricingPhase> l = new ArrayList<>();
            if (!base) l.add(new PricingPhase("Free", 0, "P1W"));
            l.add(new PricingPhase("$4.99", 4990000L, "P1M"));
            return l;
        }
    }
    public static final class PricingPhase {
        final String price, period; final long micros;
        PricingPhase(String p, long m, String per) { price = p; micros = m; period = per; }
        public String getFormattedPrice() { return price; }
        public long getPriceAmountMicros() { return micros; }
        public String getPriceCurrencyCode() { return "USD"; }
        public String getBillingPeriod() { return period; }
    }
}
