//! An in-app store screen: gems to buy again and again, a "no ads" unlock,
//! a monthly subscription, and the "Restore purchases" button Apple asks
//! for, through `platform::store`.
//!
//! On a phone it's the real store (StoreKit, Google Play) with the products
//! of the app's store listing; where there's none (a desktop, the browser)
//! it installs the mock store with a catalog of its own, so the screen works
//! anywhere. The mock scripts what the real stores do: the VIP subscription
//! waits for a parent's approval (Ask to Buy) until *Approve*, and 100 gems
//! bought "before a crash" arrive unfinished at launch.
//!
//! Every transaction comes to `App::on_transaction`, which grants it and
//! finishes it; the purchase button's callback only reports how the sheet
//! ended.
//!
//! ```sh
//! cargo run --example storedemo
//! STIPPLE_STORE=mock cargo run --example storedemo   # the mock's empty catalog
//! ```

use stipple::platform::{
    MockOutcome, MockStore, Product, ProductId, ProductKind, PurchaseResult, Store, StoreKind,
    Transaction, TransactionState, store,
};
use stipple::prelude::*;

const GEMS_100: &str = "gems_100";
const GEMS_500: &str = "gems_500";
const NO_ADS: &str = "no_ads";
const VIP: &str = "vip_monthly";

/// The products the screen sells, as the store's consoles list them.
fn product_ids() -> Vec<ProductId> {
    vec![
        ProductId::consumable(GEMS_100),
        ProductId::consumable(GEMS_500),
        ProductId::non_consumable(NO_ADS),
        ProductId::subscription(VIP),
    ]
}

/// The mock store used where there's no real one.
fn mock_catalog() -> MockStore {
    MockStore::new()
        .product(
            Product::new(GEMS_100, ProductKind::Consumable)
                .title("Pouch of gems")
                .description("100 gems")
                .price("$0.99", 990_000, "USD"),
        )
        .product(
            Product::new(GEMS_500, ProductKind::Consumable)
                .title("Chest of gems")
                .description("500 gems, 20% extra")
                .price("$3.99", 3_990_000, "USD"),
        )
        .product(
            Product::new(NO_ADS, ProductKind::NonConsumable)
                .title("No ads")
                .description("Remove the banner for good")
                .price("$2.99", 2_990_000, "USD"),
        )
        .product(
            Product::new(VIP, ProductKind::Subscription)
                .title("VIP pass")
                .description("Double daily rewards")
                .price("$4.99", 4_990_000, "USD")
                .period("P1M"),
        )
        // A parent approves the subscription (Ask to Buy).
        .outcome(VIP, MockOutcome::Pending)
        // Bought last run, but the app was killed before it finished it.
        .interrupted(GEMS_100)
}

struct Shop {
    store: StoreKind,
    /// The mock, when the app runs on one (for its Approve button).
    mock: Option<MockStore>,
    products: Vec<Product>,
    loading: bool,
    gems: u32,
    no_ads: bool,
    vip: bool,
    vip_pending: bool,
    /// The last thing that happened, for the status line.
    status: String,
    /// The transactions heard, newest first.
    log: Vec<String>,
}

impl Shop {
    fn new(store: StoreKind, mock: Option<MockStore>) -> Self {
        Self {
            store,
            mock,
            products: Vec::new(),
            loading: true,
            gems: 0,
            no_ads: false,
            vip: false,
            vip_pending: false,
            status: String::new(),
            log: Vec::new(),
        }
    }

    /// Grant what `t` bought. A real app saves the state before finishing,
    /// so a crash between the two hands the transaction back instead of
    /// losing it.
    fn grant(&mut self, t: &Transaction) {
        let how = match t.state {
            TransactionState::Purchased => "bought",
            TransactionState::Restored => "restored",
            TransactionState::Pending => "waiting for approval",
            _ => "updated",
        };
        self.log
            .insert(0, format!("{} {how} ({})", t.product_id, t.id));
        self.log.truncate(6);
        if t.state == TransactionState::Pending {
            if t.product_id == VIP {
                self.vip_pending = true;
            }
            return;
        }
        match t.product_id.as_str() {
            GEMS_100 => self.gems += 100 * t.quantity,
            GEMS_500 => self.gems += 500 * t.quantity,
            NO_ADS => self.no_ads = true,
            VIP => {
                if self.vip_pending {
                    self.status = "VIP approved".into();
                }
                self.vip = true;
                self.vip_pending = false;
            }
            _ => return,
        }
        if t.state == TransactionState::Restored {
            self.status = "Purchases restored".into();
        }
        store().finish(t);
    }
}

fn store_name(kind: StoreKind) -> &'static str {
    match kind {
        StoreKind::AppStore => "App Store",
        StoreKind::GooglePlay => "Google Play",
        StoreKind::Mock => "Mock store",
        _ => "No store",
    }
}

fn product_row(cx: &mut Cx<Shop>, state: &Shop, p: &Product) -> Element {
    let owned = (p.id == NO_ADS && state.no_ads) || (p.id == VIP && state.vip);
    let waiting = p.id == VIP && state.vip_pending;
    let period = match p.period.as_str() {
        "" => String::new(),
        "P1M" => " / month".into(),
        "P1Y" => " / year".into(),
        other => format!(" / {other}"),
    };
    let action = if owned {
        label(cx, "Owned")
    } else if waiting {
        label(cx, "Waiting…")
    } else {
        let id = p.id.clone();
        let handle = cx.handle();
        button(cx, format!("{}{period}", p.price)).on_tap(cx, move |s| {
            s.status = format!("Buying {id}…");
            handle.store().purchase(&id, |s: &mut Shop, r| {
                s.status = match r {
                    // Granted by on_transaction; nothing to do here.
                    PurchaseResult::Purchased(t) => format!("Thanks! ({})", t.product_id),
                    PurchaseResult::Pending => "Waiting for approval".into(),
                    PurchaseResult::Cancelled => "Cancelled".into(),
                    PurchaseResult::Failed(e) => format!("Failed: {e}"),
                    _ => String::new(),
                };
            });
        })
    };
    row(vec![
        column(vec![
            heading(cx, p.title.as_str()),
            label(cx, p.description.as_str()),
        ])
        .gap(2.0)
        .grow(1.0),
        action,
    ])
    .gap(12.0)
}

fn view(state: &Shop, cx: &mut Cx<Shop>) -> Element {
    let mut items = vec![
        heading(cx, "Gem shop"),
        label(
            cx,
            format!(
                "{} · {} gems{}{}",
                store_name(state.store),
                state.gems,
                if state.no_ads { " · no ads" } else { "" },
                if state.vip { " · VIP" } else { "" },
            ),
        ),
        divider(cx),
    ];
    if state.loading {
        items.push(label(cx, "Loading products…"));
    } else if state.products.is_empty() {
        items.push(label(cx, "No products available."));
    }
    let products = state.products.clone();
    for p in &products {
        items.push(product_row(cx, state, p));
    }
    items.push(divider(cx));
    let restore = button(cx, "Restore purchases").on_tap(cx, |s| {
        s.status = "Restoring…".into();
        store().restore_purchases();
    });
    let mut actions = vec![restore];
    if let Some(mock) = state.mock.clone().filter(|_| state.vip_pending) {
        actions.push(button(cx, "Approve (mock)").on_tap(cx, move |_| {
            mock.approve(VIP);
        }));
    }
    items.push(row(actions).gap(8.0));
    items.push(label(cx, state.status.as_str()));
    for line in &state.log {
        items.push(label(cx, line.as_str()));
    }
    panel(cx)
        .children(vec![column(items).gap(10.0)])
        .padding(Insets::uniform(16.0))
        .grow(1.0)
}

fn app() -> App<Shop> {
    // A real store where there's one; the mock elsewhere.
    let mock = (!store().is_available()).then(mock_catalog);
    if let Some(m) = &mock {
        Store::mock(m.clone());
    }
    let app = App::new(Shop::new(store().kind(), mock), view)
        .title("Gem shop")
        .logical_size(Size::new(480.0, 640.0))
        .on_transaction(|s: &mut Shop, t| s.grant(t));
    app.handle()
        .store()
        .products(product_ids(), |s: &mut Shop, r| {
            s.loading = false;
            match r {
                Ok(products) => s.products = products,
                Err(e) => s.status = e.to_string(),
            }
        });
    app
}

fn main() {
    app().run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buys_restores_and_approves_on_the_mock() {
        let mut app = app();
        assert_eq!(store().kind(), StoreKind::Mock);
        app.run_pending();
        assert!(app.find_text("Pouch of gems").is_some());
        assert!(app.find(Role::Button, "$4.99 / month").is_some());

        assert!(app.click(Role::Button, "$0.99"));
        app.run_pending();
        assert!(app.find_text("Mock store · 100 gems").is_some());
        assert!(app.find_text("Thanks! (gems_100)").is_some());

        assert!(app.click(Role::Button, "$2.99"));
        app.run_pending();
        assert!(app.find_text("Owned").is_some());

        // Ask to Buy: pending until approved.
        assert!(app.click(Role::Button, "$4.99 / month"));
        app.run_pending();
        assert!(app.find_text("Waiting…").is_some());
        assert!(app.click(Role::Button, "Approve (mock)"));
        app.run_pending();
        assert!(
            app.find_text("Mock store · 100 gems · no ads · VIP")
                .is_some()
        );

        // What was bought comes back: the unlock and the subscription.
        store().restore_purchases();
        app.run_pending();
        assert!(app.find_text("no_ads restored (mock-5)").is_some());
        let pm = app.render_once();
        assert!(pm.size().width > 0);
        if let Ok(path) = std::env::var("STOREDEMO_PNG") {
            pm.save_png(path).unwrap();
        }
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
