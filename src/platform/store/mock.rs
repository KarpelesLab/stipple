//! An in-memory store with scripted outcomes, for tests, CI and the desktop
//! build of a phone app.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{
    Product, ProductId, ProductKind, PurchaseResult, StoreError, StoreKind, Transaction,
    TransactionState, deliver, kind_of,
};

/// What purchasing a product of a [`MockStore`] does.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockOutcome {
    /// It's bought at once (the default).
    Purchased,
    /// It waits for approval, until [`MockStore::approve`].
    Pending,
    /// The user cancels.
    Cancelled,
    /// It fails with this error.
    Failed(StoreError),
}

/// A store in memory: products, scripted purchase outcomes, past purchases
/// to restore. Install it with [`Store::mock`](super::Store::mock) (or set
/// `STIPPLE_STORE=mock` for an empty one); it answers at once, on the
/// caller's thread, and its transactions reach
/// [`App::on_transaction`](crate::App::on_transaction) as the real stores'
/// do.
///
/// With no products, it makes one up for any id it's asked about (titled
/// with the id, at $0.99), so any app runs against it unchanged.
///
/// ```
/// use stipple::platform::{MockOutcome, MockStore, Product, ProductKind};
/// let mock = MockStore::new()
///     .product(Product::new("gems_100", ProductKind::Consumable)
///         .title("100 gems").price("$0.99", 990_000, "USD"))
///     .product(Product::new("no_ads", ProductKind::NonConsumable)
///         .title("No ads").price("$2.99", 2_990_000, "USD"))
///     .outcome("gems_100", MockOutcome::Pending)
///     .owned("no_ads");
/// assert!(mock.unfinished().is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct MockStore {
    inner: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    catalog: Vec<Product>,
    outcomes: HashMap<String, MockOutcome>,
    /// Non-consumables and subscriptions bought, for restoring.
    owned: Vec<String>,
    /// Purchases left unfinished by a "previous run", delivered at start.
    interrupted: Vec<String>,
    /// Transactions delivered and not yet finished.
    unfinished: Vec<Transaction>,
    /// Purchases waiting for [`MockStore::approve`].
    pending: Vec<String>,
    next: u64,
    started: bool,
}

impl MockStore {
    /// A mock with no products: it makes one up for any id.
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Add `product` to the catalog. Once there's one, ids not in it are
    /// unknown.
    pub fn product(self, product: Product) -> Self {
        self.with(|s| s.catalog.push(product));
        self
    }

    /// Make purchases of `product_id` end with `outcome`.
    pub fn outcome(self, product_id: impl Into<String>, outcome: MockOutcome) -> Self {
        self.with(|s| s.outcomes.insert(product_id.into(), outcome));
        self
    }

    /// Mark `product_id` as bought before, for
    /// [`Store::restore_purchases`](super::Store::restore_purchases) to bring
    /// back.
    pub fn owned(self, product_id: impl Into<String>) -> Self {
        self.with(|s| s.owned.push(product_id.into()));
        self
    }

    /// Pretend a purchase of `product_id` was paid for but never finished
    /// (the app was killed during it): its transaction arrives when the app
    /// starts listening, as the real stores hand it back at launch.
    pub fn interrupted(self, product_id: impl Into<String>) -> Self {
        self.with(|s| s.interrupted.push(product_id.into()));
        self
    }

    /// Approve the [pending](MockOutcome::Pending) purchase of `product_id`
    /// (a parent said yes): its transaction arrives, purchased. Returns
    /// whether one was waiting.
    pub fn approve(&self, product_id: &str) -> bool {
        let found = self.with(|s| {
            let at = s.pending.iter().position(|p| p == product_id)?;
            s.pending.remove(at);
            Some(s.transaction(product_id, TransactionState::Purchased))
        });
        match found {
            Some(t) => {
                deliver(t);
                true
            }
            None => false,
        }
    }

    /// The transactions delivered and not yet
    /// [finished](super::Store::finish), oldest first.
    pub fn unfinished(&self) -> Vec<Transaction> {
        self.with(|s| s.unfinished.clone())
    }

    /// The ids of the non-consumables and subscriptions owned.
    pub fn owned_products(&self) -> Vec<String> {
        self.with(|s| s.owned.clone())
    }

    pub(super) fn products(&self, ids: &[ProductId]) -> Result<Vec<Product>, StoreError> {
        Ok(self.with(|s| {
            ids.iter()
                .filter_map(|id| {
                    if s.catalog.is_empty() {
                        return Some(made_up(id));
                    }
                    s.catalog.iter().find(|p| p.id == id.id).cloned()
                })
                .collect()
        }))
    }

    pub(super) fn purchase(&self, product_id: &str) -> PurchaseResult {
        let mut pending = None;
        let result = self.with(|s| {
            if !s.catalog.is_empty() && !s.catalog.iter().any(|p| p.id == product_id) {
                return PurchaseResult::Failed(StoreError::UnknownProduct(product_id.to_string()));
            }
            let kind = s.kind(product_id);
            if kind != ProductKind::Consumable && s.owned.iter().any(|o| o == product_id) {
                return PurchaseResult::Failed(StoreError::AlreadyOwned);
            }
            match s.outcomes.get(product_id).cloned() {
                None | Some(MockOutcome::Purchased) => PurchaseResult::Purchased(
                    s.transaction(product_id, TransactionState::Purchased),
                ),
                Some(MockOutcome::Pending) => {
                    s.pending.push(product_id.to_string());
                    // Reported as the real stores do, without a handle: it
                    // can't be finished.
                    pending = Some(s.transaction(product_id, TransactionState::Pending));
                    PurchaseResult::Pending
                }
                Some(MockOutcome::Cancelled) => PurchaseResult::Cancelled,
                Some(MockOutcome::Failed(e)) => PurchaseResult::Failed(e),
            }
        });
        if let PurchaseResult::Purchased(t) = &result {
            deliver(t.clone());
        }
        if let Some(t) = pending {
            deliver(t);
        }
        result
    }

    pub(super) fn finish(&self, t: &Transaction) {
        self.with(|s| {
            s.unfinished.retain(|u| u.handle != t.handle);
            if t.kind != ProductKind::Consumable && !s.owned.contains(&t.product_id) {
                s.owned.push(t.product_id.clone());
            }
        });
    }

    pub(super) fn restore(&self) {
        let restored = self.with(|s| {
            let owned = s.owned.clone();
            owned
                .iter()
                .map(|id| s.transaction(id, TransactionState::Restored))
                .collect::<Vec<_>>()
        });
        for t in restored {
            deliver(t);
        }
    }

    pub(super) fn start(&self) {
        let interrupted = self.with(|s| {
            if std::mem::replace(&mut s.started, true) {
                return Vec::new();
            }
            let ids = std::mem::take(&mut s.interrupted);
            ids.iter()
                .map(|id| s.transaction(id, TransactionState::Purchased))
                .collect()
        });
        for t in interrupted {
            deliver(t);
        }
    }
}

impl State {
    fn kind(&self, id: &str) -> ProductKind {
        self.catalog
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.kind)
            .or_else(|| kind_of(id))
            .unwrap_or(ProductKind::NonConsumable)
    }

    /// A new transaction of `product_id`, kept as unfinished.
    fn transaction(&mut self, product_id: &str, state: TransactionState) -> Transaction {
        self.next += 1;
        let id = format!("mock-{}", self.next);
        let t = Transaction {
            product_id: product_id.to_string(),
            kind: self.kind(product_id),
            state,
            original_id: id.clone(),
            token: format!("mock-token-{}", self.next),
            receipt: format!("{{\"productId\":\"{product_id}\",\"orderId\":\"{id}\"}}"),
            signature: String::new(),
            id,
            quantity: 1,
            purchase_time_ms: now_ms(),
            store: StoreKind::Mock,
            handle: self.next,
        };
        if state != TransactionState::Pending {
            self.unfinished.push(t.clone());
        }
        t
    }
}

/// A product for `id` when the mock has no catalog.
fn made_up(id: &ProductId) -> Product {
    let p = Product::new(id.id.clone(), id.kind)
        .title(id.id.clone())
        .description(format!("{} (mock store)", id.id))
        .price("$0.99", 990_000, "USD");
    if id.kind == ProductKind::Subscription {
        p.period("P1M")
    } else {
        p
    }
}

fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}
