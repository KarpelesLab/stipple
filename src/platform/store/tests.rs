use super::*;
use std::sync::Arc;

/// The tests share the process's one store.
static SERIAL: Mutex<()> = Mutex::new(());

fn catalog() -> MockStore {
    MockStore::new()
        .product(
            Product::new("gems", ProductKind::Consumable)
                .title("100 gems")
                .price("$0.99", 990_000, "USD"),
        )
        .product(
            Product::new("no_ads", ProductKind::NonConsumable)
                .title("No ads")
                .price("$2.99", 2_990_000, "USD"),
        )
        .product(
            Product::new("vip", ProductKind::Subscription)
                .title("VIP")
                .price("$4.99", 4_990_000, "USD")
                .period("P1M"),
        )
}

/// Run `f` with its answer captured.
fn answer<T: Send + 'static>(f: impl FnOnce(Box<dyn FnOnce(T) + Send>)) -> T {
    let got = Arc::new(Mutex::new(None));
    let g = got.clone();
    f(Box::new(move |v| *g.lock().unwrap() = Some(v)));
    got.lock().unwrap().take().expect("answered at once")
}

fn transactions() -> Vec<Transaction> {
    crate::platform::shell::take_posted()
        .into_iter()
        .filter_map(|e| match e {
            Event::Transaction(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn the_mock_lists_buys_finishes_and_restores() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mock = catalog().outcome("vip", MockOutcome::Pending);
    let store = Store::mock(mock.clone());
    assert_eq!(store.kind(), StoreKind::Mock);
    assert!(store.is_available());

    // Unknown ids are left out, as the stores do.
    let products = answer(|done| {
        store.products(
            &[
                ProductId::consumable("gems"),
                ProductId::non_consumable("nope"),
                ProductId::subscription("vip"),
            ],
            done,
        )
    })
    .unwrap();
    let ids: Vec<_> = products.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["gems", "vip"]);
    assert_eq!(products[1].period, "P1M");

    // A consumable: bought, delivered, finished, buyable again.
    let r = answer(|done| store.purchase("gems", done));
    let PurchaseResult::Purchased(t) = r else {
        panic!("{r:?}")
    };
    assert_eq!(t.kind, ProductKind::Consumable);
    assert_eq!(mock.unfinished(), std::slice::from_ref(&t));
    store.finish(&t);
    assert!(mock.unfinished().is_empty());
    assert!(matches!(
        answer(|done| store.purchase("gems", done)),
        PurchaseResult::Purchased(_)
    ));
    for t in mock.unfinished() {
        store.finish(&t);
    }

    // A non-consumable: owned once finished, then refused and restorable.
    let PurchaseResult::Purchased(t) = answer(|done| store.purchase("no_ads", done)) else {
        panic!()
    };
    store.finish(&t);
    assert_eq!(mock.owned_products(), ["no_ads"]);
    assert_eq!(
        answer(|done| store.purchase("no_ads", done)),
        PurchaseResult::Failed(StoreError::AlreadyOwned)
    );
    let _ = transactions();
    store.restore_purchases();
    let restored = mock.unfinished();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].state, TransactionState::Restored);
    assert!(restored[0].is_paid());
    store.finish(&restored[0]);

    // Pending until approved; finishing a pending one does nothing.
    assert_eq!(
        answer(|done| store.purchase("vip", done)),
        PurchaseResult::Pending
    );
    assert!(mock.unfinished().is_empty());
    assert!(mock.approve("vip"));
    assert!(!mock.approve("vip"));
    let approved = mock.unfinished();
    assert_eq!(approved[0].kind, ProductKind::Subscription);
    assert_eq!(approved[0].state, TransactionState::Purchased);

    // Unknown products can't be bought.
    assert_eq!(
        answer(|done| store.purchase("nope", done)),
        PurchaseResult::Failed(StoreError::UnknownProduct("nope".into()))
    );
    *MOCK.lock().unwrap() = None;
}

#[test]
fn scripted_outcomes_and_interrupted_purchases() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mock = MockStore::new()
        .outcome("a", MockOutcome::Cancelled)
        .outcome("b", MockOutcome::Failed(StoreError::Network))
        .interrupted("c");
    let store = Store::mock(mock.clone());
    assert_eq!(
        answer(|done| store.purchase("a", done)),
        PurchaseResult::Cancelled
    );
    assert_eq!(
        answer(|done| store.purchase("b", done)),
        PurchaseResult::Failed(StoreError::Network)
    );
    // With no catalog, any id is a product.
    let made = answer(|done| store.products(&[ProductId::subscription("z")], done)).unwrap();
    assert_eq!(made[0].price, "$0.99");
    assert_eq!(made[0].period, "P1M");
    // The interrupted purchase arrives once the app listens, once.
    assert!(mock.unfinished().is_empty());
    store.start();
    store.start();
    let left = mock.unfinished();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].product_id, "c");
    *MOCK.lock().unwrap() = None;
}

#[test]
fn without_a_store_everything_fails_softly() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if std::env::var("STIPPLE_STORE").is_ok() {
        return;
    }
    *MOCK.lock().unwrap() = None;
    let store = store();
    assert_eq!(store.kind(), StoreKind::Unavailable);
    let r = answer(|done| store.products(&[ProductId::consumable("x")], done));
    assert!(matches!(r, Err(StoreError::Unavailable(_))));
    let r = answer(|done| store.purchase("x", done));
    assert!(matches!(
        r,
        PurchaseResult::Failed(StoreError::Unavailable(_))
    ));
    store.restore_purchases();
    store.finish(&Transaction::new("x", ProductKind::Consumable, "t"));
    assert_eq!(
        StoreError::UnknownProduct("x".into()).to_string(),
        "unknown product \"x\""
    );
}
