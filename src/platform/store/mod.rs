//! In-app purchases: [`store`] is the app store's purchase system, the same
//! shape on both phones.
//!
//! | | How | [`Transaction::token`] | [`Transaction::receipt`] |
//! |---|---|---|---|
//! | iOS | StoreKit (the original API: `SKProductsRequest`, `SKPaymentQueue` and a transaction observer, through the Objective-C runtime) | the transaction identifier | the app receipt, base64 |
//! | Android | the Google Play Billing Library (6 to 9), driven over JNI (no Java of Stipple's; the library must be in the APK) | the purchase token | the purchase's signed JSON |
//! | Desktops, web | — ([`StoreError::Unavailable`]) | | |
//! | Anywhere | [`Store::mock`], or `STIPPLE_STORE=mock` | made up | made up |
//!
//! Purchases are asynchronous and may outlive the process (the sheet is the
//! system's; a payment approved by a parent arrives days later; the app may
//! be killed mid-purchase). So every transaction reaches the app through one
//! place, [`App::on_transaction`](crate::App::on_transaction): the app grants
//! what was bought there, then calls [`Store::finish`]. The callback of
//! [`Store::purchase`] only says how that attempt ended, for the UI.
//!
//! See `docs/purchases.md`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use crate::platform::event::Event;

mod mock;
pub use mock::{MockOutcome, MockStore};

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) mod ios;

/// What a product is to the app, which decides what [`Store::finish`] does
/// with its transactions. Neither store's original API says, so the app does,
/// with each [`ProductId`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProductKind {
    /// Bought again and again, used up (gems, coins): finishing a transaction
    /// consumes it on Google Play, so it can be bought again.
    Consumable,
    /// Bought once, owned for good (remove ads, a level pack): restored by
    /// [`Store::restore_purchases`]. Finishing acknowledges it on Google Play.
    NonConsumable,
    /// Renews until cancelled. Finishing acknowledges it on Google Play.
    Subscription,
}

/// A product to ask the store about: its identifier in App Store Connect or
/// the Play Console, and its kind.
///
/// ```
/// use stipple::platform::{ProductId, ProductKind};
/// let ids = [
///     ProductId::consumable("gems_100"),
///     ProductId::non_consumable("no_ads"),
///     ProductId::subscription("vip_monthly"),
/// ];
/// assert_eq!(ids[1].kind, ProductKind::NonConsumable);
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProductId {
    /// The store's identifier.
    pub id: String,
    /// What it is.
    pub kind: ProductKind,
}

impl ProductId {
    /// Product `id`, of kind `kind`.
    pub fn new(id: impl Into<String>, kind: ProductKind) -> Self {
        Self {
            id: id.into(),
            kind,
        }
    }

    /// A consumable product.
    pub fn consumable(id: impl Into<String>) -> Self {
        Self::new(id, ProductKind::Consumable)
    }

    /// A non-consumable product.
    pub fn non_consumable(id: impl Into<String>) -> Self {
        Self::new(id, ProductKind::NonConsumable)
    }

    /// An auto-renewing subscription.
    pub fn subscription(id: impl Into<String>) -> Self {
        Self::new(id, ProductKind::Subscription)
    }
}

/// A product as the store describes it, in the user's language and currency.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    /// The store's identifier.
    pub id: String,
    /// What it is (as the app said in its [`ProductId`]).
    pub kind: ProductKind,
    /// Its name, as entered in the store's console, localized.
    pub title: String,
    /// Its description, localized.
    pub description: String,
    /// The price as the store formats it for the user: `$0.99`, `120 円`.
    /// Show this, not a price formatted from the numbers.
    pub price: String,
    /// The price in millionths of the currency's unit: 990000 for 0.99.
    pub price_micros: i64,
    /// The price's ISO 4217 currency code: `USD`.
    pub currency: String,
    /// A subscription's billing period as ISO 8601 (`P1M`, `P1Y`); empty for
    /// other products.
    pub period: String,
}

impl Product {
    /// A product with no title, description or price yet: for
    /// [`MockStore::product`] and tests.
    pub fn new(id: impl Into<String>, kind: ProductKind) -> Self {
        Self {
            id: id.into(),
            kind,
            title: String::new(),
            description: String::new(),
            price: String::new(),
            price_micros: 0,
            currency: String::new(),
            period: String::new(),
        }
    }

    /// Set the title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set the price: as shown (`$0.99`), in micros (990000) and its currency
    /// (`USD`).
    pub fn price(
        mut self,
        shown: impl Into<String>,
        micros: i64,
        currency: impl Into<String>,
    ) -> Self {
        self.price = shown.into();
        self.price_micros = micros;
        self.currency = currency.into();
        self
    }

    /// Set a subscription's period (ISO 8601: `P1M`).
    pub fn period(mut self, period: impl Into<String>) -> Self {
        self.period = period.into();
        self
    }
}

/// Where a [`Transaction`] stands.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransactionState {
    /// Paid for: grant it, then [`finish`](Store::finish) it.
    Purchased,
    /// Bought before (on this or another device), brought back by
    /// [`Store::restore_purchases`]: grant it again, then finish it.
    Restored,
    /// Waiting: for a parent's approval (Ask to Buy), or for a payment in
    /// cash at a shop (Google Play's pending purchases). Grant nothing yet;
    /// the same purchase comes back as [`Purchased`](Self::Purchased), or
    /// never.
    Pending,
}

/// A purchase, reported to [`App::on_transaction`](crate::App::on_transaction).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// The product bought.
    pub product_id: String,
    /// Its kind, if the app named it in a [`ProductId`] this run (with
    /// [`Store::products`]); otherwise a guess that never loses a purchase:
    /// [`ProductKind::NonConsumable`] for a one-time product (finishing
    /// acknowledges it without consuming it), [`ProductKind::Subscription`]
    /// for a subscription Google Play says is one. [`Store::finish`] goes by
    /// this field: ask for the products before transactions arrive, or set
    /// it before finishing.
    pub kind: ProductKind,
    /// Where it stands.
    pub state: TransactionState,
    /// The transaction's identifier: `transactionIdentifier` on iOS, the
    /// order id on Google Play (the purchase token when it has none).
    pub id: String,
    /// The identifier of the first purchase this one restores or renews;
    /// equal to [`id`](Self::id) otherwise.
    pub original_id: String,
    /// How many were bought.
    pub quantity: u32,
    /// When, in milliseconds since the Unix epoch (0 when not known).
    pub purchase_time_ms: u64,
    /// What a server checks the purchase with: on Google Play the purchase
    /// token (for the Play Developer API's `purchases.products.get` or
    /// `purchases.subscriptionsv2.get`); on iOS the transaction identifier
    /// (for the App Store Server API's `Get Transaction Info`).
    pub token: String,
    /// The signed proof of purchase: on Google Play the purchase's original
    /// JSON (signed by [`signature`](Self::signature)); on iOS the app
    /// receipt, base64 (for the older `verifyReceipt` endpoint).
    pub receipt: String,
    /// Google Play's signature of [`receipt`](Self::receipt), base64
    /// (RSA-SHA1 with the app's licensing key); empty elsewhere.
    pub signature: String,
    /// Which store it came from.
    pub store: StoreKind,
    /// The backend's handle on it, for [`Store::finish`].
    pub(crate) handle: u64,
}

impl Transaction {
    /// A purchased transaction of `product_id`, for tests.
    pub fn new(product_id: impl Into<String>, kind: ProductKind, id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            product_id: product_id.into(),
            kind,
            state: TransactionState::Purchased,
            original_id: id.clone(),
            token: id.clone(),
            id,
            quantity: 1,
            purchase_time_ms: 0,
            receipt: String::new(),
            signature: String::new(),
            store: StoreKind::Mock,
            handle: 0,
        }
    }

    /// Whether the app should grant it now: purchased or restored.
    pub fn is_paid(&self) -> bool {
        matches!(
            self.state,
            TransactionState::Purchased | TransactionState::Restored
        )
    }
}

/// How a [`Store::purchase`] ended.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurchaseResult {
    /// Bought. The same transaction also goes to
    /// [`App::on_transaction`](crate::App::on_transaction), which grants it:
    /// don't grant it here as well.
    Purchased(Transaction),
    /// Waiting for approval or payment ([`TransactionState::Pending`]); the
    /// outcome arrives later, at [`App::on_transaction`](crate::App::on_transaction).
    Pending,
    /// The user closed the sheet.
    Cancelled,
    /// It didn't go through.
    Failed(StoreError),
}

/// Which store a [`Store`] talks to.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreKind {
    /// Apple's App Store (StoreKit).
    AppStore,
    /// Google Play (the Play Billing Library).
    GooglePlay,
    /// [`MockStore`], in memory.
    Mock,
    /// None: a desktop, the web, an Android app without the Play Billing
    /// Library.
    Unavailable,
}

/// Why the store couldn't do something.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// There's no store here: a desktop, the web, an APK without the Play
    /// Billing Library, a device without Google Play or signed out of it.
    /// The text says which.
    Unavailable(String),
    /// The user may not pay (parental controls, a device policy).
    NotAllowed,
    /// The store doesn't know the product (wrong id, not approved yet, or the
    /// app isn't set up for purchases in the store's console).
    UnknownProduct(String),
    /// A non-consumable or subscription the user already owns: restore it
    /// instead (the store sends it to `on_transaction` as well, where it can).
    AlreadyOwned,
    /// Another purchase is in progress.
    Busy,
    /// The store couldn't be reached.
    Network,
    /// Anything else, as the store worded it.
    Store(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Unavailable(why) => write!(f, "no store available: {why}"),
            StoreError::NotAllowed => f.write_str("purchases aren't allowed on this device"),
            StoreError::UnknownProduct(id) => write!(f, "unknown product {id:?}"),
            StoreError::AlreadyOwned => f.write_str("already owned"),
            StoreError::Busy => f.write_str("another purchase is in progress"),
            StoreError::Network => f.write_str("the store couldn't be reached"),
            StoreError::Store(msg) => write!(f, "store error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The app's store: products, purchases and transactions. Get it with
/// [`store`]; it's a cheap handle on the process's one store, callable from
/// any thread.
///
/// Its callbacks run later, on whichever thread the store answers on (the UI
/// thread on iOS, Java's main thread on Android, the caller's for the mock).
/// To change the app's state from them, use
/// [`AppHandle::store`](crate::core::AppHandle::store), which runs them as
/// state updates.
#[derive(Clone, Copy, Debug)]
pub struct Store {
    _private: (),
}

/// The app's store: StoreKit on iOS, Google Play on Android, the
/// [mock](Store::mock) if one is installed (or `STIPPLE_STORE=mock` is set),
/// and none ([`StoreKind::Unavailable`]) elsewhere.
///
/// ```
/// use stipple::platform::{ProductId, store};
/// store().products(&[ProductId::consumable("gems_100")], |result| {
///     match result {
///         Ok(products) => println!("{} products", products.len()),
///         Err(e) => println!("no store: {e}"),
///     }
/// });
/// ```
pub fn store() -> Store {
    Store { _private: () }
}

/// The mock installed by [`Store::mock`] (or the environment), if any.
static MOCK: Mutex<Option<MockStore>> = Mutex::new(None);

/// What the app said its products are, by id: the kinds
/// [`Store::finish`] goes by.
static KINDS: Mutex<Option<HashMap<String, ProductKind>>> = Mutex::new(None);

/// The mock, if one is installed, or `STIPPLE_STORE=mock` asks for one.
fn mock() -> Option<MockStore> {
    let mut m = MOCK.lock().unwrap_or_else(|e| e.into_inner());
    if m.is_none() && std::env::var("STIPPLE_STORE").is_ok_and(|v| v == "mock") {
        *m = Some(MockStore::new());
    }
    m.clone()
}

/// Remember the kinds of `ids`.
fn remember(ids: &[ProductId]) {
    let mut k = KINDS.lock().unwrap_or_else(|e| e.into_inner());
    let map = k.get_or_insert_with(HashMap::new);
    for id in ids {
        map.insert(id.id.clone(), id.kind);
    }
}

/// The kind the app gave product `id`, if it did.
pub(crate) fn kind_of(id: &str) -> Option<ProductKind> {
    KINDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(id)
        .copied()
}

/// Hand `transaction` to the app's [`App::on_transaction`](crate::App::on_transaction).
/// Safe from any thread.
pub(crate) fn deliver(transaction: Transaction) {
    crate::platform::shell::post(Event::Transaction(transaction));
}

pub(crate) type ProductsFn = Box<dyn FnOnce(Result<Vec<Product>, StoreError>) + Send>;
pub(crate) type PurchaseFn = Box<dyn FnOnce(PurchaseResult) + Send>;

impl Store {
    /// Install `mock` as the process's store, in place of the real one, and
    /// return it: for tests, CI, the desktop build of a phone app, and
    /// screenshots. Keep a clone of `mock` to drive and inspect it
    /// ([`MockStore::approve`], [`MockStore::unfinished`]).
    ///
    /// ```
    /// use stipple::platform::{MockStore, Product, ProductKind, Store, StoreKind};
    /// let store = Store::mock(MockStore::new().product(
    ///     Product::new("gems_100", ProductKind::Consumable)
    ///         .title("100 gems")
    ///         .price("$0.99", 990_000, "USD"),
    /// ));
    /// assert_eq!(store.kind(), StoreKind::Mock);
    /// ```
    pub fn mock(mock: MockStore) -> Store {
        *MOCK.lock().unwrap_or_else(|e| e.into_inner()) = Some(mock);
        store()
    }

    /// Which store this is.
    pub fn kind(&self) -> StoreKind {
        if mock().is_some() {
            StoreKind::Mock
        } else {
            os::KIND
        }
    }

    /// Whether there's a store at all ([`kind`](Self::kind) isn't
    /// [`StoreKind::Unavailable`]). On Android, whether the Play Billing
    /// Library is in the APK and Google Play answers is only known once
    /// [`products`](Self::products) does.
    pub fn is_available(&self) -> bool {
        self.kind() != StoreKind::Unavailable
    }

    /// Ask the store about `ids`, and give `done` the products it knows, in
    /// the user's language and currency. Products it doesn't know are left
    /// out (as both stores do), so compare the ids. Stores remember the kinds,
    /// for [`finish`](Self::finish).
    pub fn products(
        &self,
        ids: &[ProductId],
        done: impl FnOnce(Result<Vec<Product>, StoreError>) + Send + 'static,
    ) {
        remember(ids);
        if let Some(m) = mock() {
            done(m.products(ids));
            return;
        }
        os::products(ids.to_vec(), Box::new(done));
    }

    /// Show the store's purchase sheet for `product_id`, and tell `done` how
    /// it ended. Call it from the handler of the button the user pressed.
    ///
    /// The transaction itself goes to
    /// [`App::on_transaction`](crate::App::on_transaction), where the app
    /// grants it; `done` is for the UI (close a spinner, say it failed).
    /// Asking for the product with [`products`](Self::products) first makes
    /// the sheet open faster and tells the store its kind.
    pub fn purchase(&self, product_id: &str, done: impl FnOnce(PurchaseResult) + Send + 'static) {
        if let Some(m) = mock() {
            done(m.purchase(product_id));
            return;
        }
        os::purchase(product_id.to_string(), Box::new(done));
    }

    /// Tell the store the app has granted `transaction`: StoreKit's
    /// `finishTransaction:`; on Google Play, consuming a
    /// [consumable](ProductKind::Consumable) (so it can be bought again) or
    /// acknowledging anything else. Until then the store hands the
    /// transaction back at every launch, and Google Play refunds a purchase
    /// that isn't acknowledged within three days. Finishing a
    /// [pending](TransactionState::Pending) one does nothing.
    pub fn finish(&self, transaction: &Transaction) {
        if transaction.state == TransactionState::Pending {
            return;
        }
        if let Some(m) = mock() {
            m.finish(transaction);
            return;
        }
        os::finish(transaction);
    }

    /// Ask the store for what the user bought before (non-consumables and
    /// active subscriptions, on this or another device): each comes to
    /// [`App::on_transaction`](crate::App::on_transaction) as
    /// [`TransactionState::Restored`]. Apple requires a button for it.
    /// iOS may ask the user to sign in.
    pub fn restore_purchases(&self) {
        if let Some(m) = mock() {
            m.restore();
            return;
        }
        os::restore();
    }

    /// Start listening for transactions: the ones left unfinished by an
    /// earlier run, or approved since. [`App::run`](crate::App::run) calls it
    /// when the app has an [`on_transaction`](crate::App::on_transaction)
    /// hook.
    pub(crate) fn start(&self) {
        if let Some(m) = mock() {
            m.start();
            return;
        }
        os::start();
    }
}

// ---- Per-platform dispatch -------------------------------------------------

#[cfg(target_os = "android")]
mod os {
    pub(super) use crate::platform::backend::android::billing::{
        KIND, finish, products, purchase, restore, start,
    };
}

#[cfg(target_os = "ios")]
mod os {
    pub(super) use super::ios::{KIND, finish, products, purchase, restore, start};
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod os {
    use super::*;

    pub(super) const KIND: StoreKind = StoreKind::Unavailable;

    fn why() -> StoreError {
        StoreError::Unavailable(if cfg!(target_arch = "wasm32") {
            "no app store on the web".into()
        } else {
            "no app store on this platform".into()
        })
    }

    pub(super) fn products(_ids: Vec<ProductId>, done: ProductsFn) {
        done(Err(why()));
    }

    pub(super) fn purchase(_id: String, done: PurchaseFn) {
        done(PurchaseResult::Failed(why()));
    }

    pub(super) fn finish(_t: &Transaction) {}

    pub(super) fn restore() {}

    pub(super) fn start() {}
}

#[cfg(test)]
mod tests;
