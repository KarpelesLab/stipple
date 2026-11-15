//! iOS: StoreKit's original API, through the Objective-C runtime.
//!
//! StoreKit 2 (`Product`, `Transaction`) is Swift-only: its types are Swift
//! structs and actors with no Objective-C face. The original API (deprecated
//! in the iOS 18 SDK, still shipped and working, and accepted by App Review)
//! is plain Objective-C: an `SKProductsRequest` answers which products exist
//! and their localized prices, `SKPaymentQueue` takes payments, and a
//! transaction observer hears every transaction (the queue hands back the
//! unfinished ones each time an observer is added, so a purchase interrupted
//! by a crash comes back at the next launch). Its transactions are the same
//! App Store transactions StoreKit 2 sees: their identifiers work with the
//! App Store Server API.
//!
//! One object of a class built at run time (`StippleStoreObserver`) is both
//! the queue's observer and the requests' delegate. StoreKit calls it on the
//! main thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    Product, ProductId, ProductKind, ProductsFn, PurchaseFn, PurchaseResult, StoreError, StoreKind,
    Transaction, TransactionState, deliver, kind_of,
};
use crate::platform::native::objc::{Id, Sel, class, msg, nsstring, rust_string, sel, with_pool};

pub(crate) const KIND: StoreKind = StoreKind::AppStore;

// Linking the framework makes its classes known.
#[link(name = "StoreKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
}

// `SKPaymentTransactionState`.
const PURCHASED: isize = 1;
const FAILED: isize = 2;
const RESTORED: isize = 3;
const DEFERRED: isize = 4;

// `SKErrorCode`.
const SK_PAYMENT_CANCELLED: isize = 2;
const SK_PAYMENT_NOT_ALLOWED: isize = 4;
const SK_PRODUCT_NOT_AVAILABLE: isize = 5;
const SK_NETWORK_FAILED: isize = 7;

/// What's waiting on StoreKit, by the addresses of its objects.
#[derive(Default)]
struct State {
    /// Product requests in flight: the request, the ids asked for, the answer.
    requests: Vec<(usize, Vec<ProductId>, ProductsFn)>,
    /// Retained `SKProduct`s, by id: what a payment is made from.
    products: HashMap<String, usize>,
    /// Retained `SKPaymentTransaction`s handed to the app, by handle, until
    /// it finishes them.
    transactions: HashMap<u64, usize>,
    next: u64,
    /// The purchase in progress: its product and its answer.
    purchase: Option<(String, PurchaseFn)>,
}

// SAFETY: the addresses are of Objective-C objects only messaged from here.
static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(State::default))
}

/// Whether the app finished launching, and whether it asked to listen before.
static LAUNCHED: AtomicBool = AtomicBool::new(false);
static WANTED: AtomicBool = AtomicBool::new(false);

/// The observer and delegate, made once.
fn observer() -> Id {
    static OBSERVER: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *OBSERVER.get_or_init(|| unsafe {
        let name = c"StippleStoreObserver";
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            let methods: [(&str, *const c_void, &std::ffi::CStr); 5] = [
                (
                    "paymentQueue:updatedTransactions:",
                    updated_transactions as *const c_void,
                    c"v@:@@",
                ),
                (
                    "paymentQueue:restoreCompletedTransactionsFailedWithError:",
                    restore_failed as *const c_void,
                    c"v@:@@",
                ),
                (
                    "productsRequest:didReceiveResponse:",
                    did_receive_response as *const c_void,
                    c"v@:@@",
                ),
                (
                    "request:didFailWithError:",
                    request_failed as *const c_void,
                    c"v@:@@",
                ),
                (
                    "requestDidFinish:",
                    request_finished as *const c_void,
                    c"v@:@",
                ),
            ];
            for (selector, imp, types) in methods {
                class_addMethod(cls, sel(selector), imp, types.as_ptr());
            }
            for protocol in [
                c"SKPaymentTransactionObserver",
                c"SKProductsRequestDelegate",
            ] {
                let p = objc_getProtocol(protocol.as_ptr());
                if !p.is_null() {
                    class_addProtocol(cls, p);
                }
            }
            objc_registerClassPair(cls);
        }
        msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) as usize
    }) as Id
}

/// `[SKPaymentQueue defaultQueue]`.
fn queue() -> Id {
    unsafe { msg!(class("SKPaymentQueue"), "defaultQueue", () -> Id) }
}

/// Add the observer to the payment queue, once: from then on StoreKit hands
/// it every transaction, the unfinished ones of earlier runs first.
fn observe() {
    static ADDED: AtomicBool = AtomicBool::new(false);
    if ADDED.swap(true, Ordering::AcqRel) {
        return;
    }
    with_pool(|| unsafe {
        let observer = observer();
        msg!(queue(), "addTransactionObserver:", (observer: Id) -> ());
    });
}

pub(crate) fn start() {
    if LAUNCHED.load(Ordering::Acquire) {
        observe();
    } else {
        WANTED.store(true, Ordering::Release);
    }
}

/// The app finished launching: listen now if the app asked to before.
pub(crate) fn launched() {
    LAUNCHED.store(true, Ordering::Release);
    if WANTED.load(Ordering::Acquire) {
        observe();
    }
}

pub(crate) fn products(ids: Vec<ProductId>, done: ProductsFn) {
    observe();
    with_pool(|| unsafe {
        let array = msg!(class("NSMutableArray"), "array", () -> Id);
        for id in &ids {
            let s = nsstring(&id.id);
            msg!(array, "addObject:", (s: Id) -> ());
        }
        let set = msg!(class("NSSet"), "setWithArray:", (array: Id) -> Id);
        let request = msg!(class("SKProductsRequest"), "alloc", () -> Id);
        let request = msg!(request, "initWithProductIdentifiers:", (set: Id) -> Id);
        if request.is_null() {
            done(Err(StoreError::Store(
                "StoreKit refused the request".into(),
            )));
            return;
        }
        let observer = observer();
        msg!(request, "setDelegate:", (observer: Id) -> ());
        with(|s| s.requests.push((request as usize, ids, done)));
        msg!(request, "start", () -> ());
    });
}

/// The request at `request`'s entry, taken out (and the request released).
fn take_request(request: Id) -> Option<(Vec<ProductId>, ProductsFn)> {
    let found = with(|s| {
        let at = s.requests.iter().position(|r| r.0 == request as usize)?;
        let (_, ids, done) = s.requests.remove(at);
        Some((ids, done))
    });
    if found.is_some() {
        unsafe { msg!(request, "release", () -> ()) };
    }
    found
}

/// `productsRequest:didReceiveResponse:`: the products StoreKit knows.
extern "C" fn did_receive_response(_this: Id, _cmd: Sel, request: Id, response: Id) {
    let Some((ids, done)) = take_request(request) else {
        return;
    };
    let mut out = Vec::new();
    unsafe {
        let list = msg!(response, "products", () -> Id);
        for i in 0..count(list) {
            let p = msg!(list, "objectAtIndex:", (i: usize) -> Id);
            let Some(product) = product(p, &ids) else {
                continue;
            };
            msg!(p, "retain", () -> Id);
            if let Some(old) = with(|s| s.products.insert(product.id.clone(), p as usize)) {
                msg!(old as Id, "release", () -> ());
            }
            out.push(product);
        }
        let invalid = msg!(response, "invalidProductIdentifiers", () -> Id);
        for i in 0..count(invalid) {
            let id = rust_string(msg!(invalid, "objectAtIndex:", (i: usize) -> Id));
            eprintln!("stipple: StoreKit doesn't know product {id:?}");
        }
    }
    done(Ok(out));
}

/// `request:didFailWithError:`.
extern "C" fn request_failed(_this: Id, _cmd: Sel, request: Id, error: Id) {
    if let Some((_, done)) = take_request(request) {
        done(Err(store_error(error)));
    }
}

/// `requestDidFinish:`: nothing to do (the response came first).
extern "C" fn request_finished(_this: Id, _cmd: Sel, _request: Id) {}

fn count(array: Id) -> usize {
    if array.is_null() {
        0
    } else {
        unsafe { msg!(array, "count", () -> usize) }
    }
}

/// An `SKProduct` as a [`Product`], with the kind the app asked for.
unsafe fn product(p: Id, ids: &[ProductId]) -> Option<Product> {
    unsafe {
        let id = rust_string(msg!(p, "productIdentifier", () -> Id))?;
        let kind = ids
            .iter()
            .find(|i| i.id == id)
            .map_or(ProductKind::NonConsumable, |i| i.kind);
        let price = msg!(p, "price", () -> Id);
        let locale = msg!(p, "priceLocale", () -> Id);
        let formatter = msg!(msg!(class("NSNumberFormatter"), "alloc", () -> Id), "init", () -> Id);
        // NSNumberFormatterBehavior10_4, NSNumberFormatterCurrencyStyle.
        let (behavior, style) = (1040usize, 2usize);
        msg!(formatter, "setFormatterBehavior:", (behavior: usize) -> ());
        msg!(formatter, "setNumberStyle:", (style: usize) -> ());
        msg!(formatter, "setLocale:", (locale: Id) -> ());
        let shown = rust_string(msg!(formatter, "stringFromNumber:", (price: Id) -> Id));
        msg!(formatter, "release", () -> ());
        let micros = if price.is_null() {
            0
        } else {
            let power = 6i16;
            let scaled = msg!(price, "decimalNumberByMultiplyingByPowerOf10:", (power: i16) -> Id);
            msg!(scaled, "longLongValue", () -> i64)
        };
        let currency = if locale.is_null() {
            None
        } else {
            rust_string(msg!(locale, "currencyCode", () -> Id))
        };
        let mut out = Product::new(id, kind)
            .title(rust_string(msg!(p, "localizedTitle", () -> Id)).unwrap_or_default())
            .description(rust_string(msg!(p, "localizedDescription", () -> Id)).unwrap_or_default())
            .price(
                shown.unwrap_or_default(),
                micros,
                currency.unwrap_or_default(),
            );
        // `subscriptionPeriod` (iOS 11.2): nil for other products.
        let selector = sel("subscriptionPeriod");
        let responds = msg!(p, "respondsToSelector:", (selector: Sel) -> bool);
        let period = if responds {
            msg!(p, "subscriptionPeriod", () -> Id)
        } else {
            std::ptr::null_mut()
        };
        if !period.is_null() {
            let n = msg!(period, "numberOfUnits", () -> usize);
            let unit = match msg!(period, "unit", () -> usize) {
                0 => 'D',
                1 => 'W',
                2 => 'M',
                _ => 'Y',
            };
            out = out.period(format!("P{n}{unit}"));
            // Not one the app named: a subscription, as StoreKit says.
            if ids.iter().all(|i| i.id != out.id) {
                out.kind = ProductKind::Subscription;
            }
        }
        Some(out)
    }
}

pub(crate) fn purchase(product_id: String, done: PurchaseFn) {
    observe();
    let can = unsafe { msg!(class("SKPaymentQueue"), "canMakePayments", () -> bool) };
    if !can {
        done(PurchaseResult::Failed(StoreError::NotAllowed));
        return;
    }
    if with(|s| s.purchase.is_some()) {
        done(PurchaseResult::Failed(StoreError::Busy));
        return;
    }
    let product = with(|s| s.products.get(&product_id).copied());
    let Some(product) = product else {
        // Not asked about yet: ask, then buy.
        let kind = kind_of(&product_id).unwrap_or(ProductKind::NonConsumable);
        let id = product_id.clone();
        products(
            vec![ProductId::new(product_id, kind)],
            Box::new(move |r| match r {
                Ok(list) if list.iter().any(|p| p.id == id) => purchase(id, done),
                Ok(_) => done(PurchaseResult::Failed(StoreError::UnknownProduct(id))),
                Err(e) => done(PurchaseResult::Failed(e)),
            }),
        );
        return;
    };
    with(|s| s.purchase = Some((product_id, done)));
    with_pool(|| unsafe {
        let product = product as Id;
        let payment = msg!(class("SKPayment"), "paymentWithProduct:", (product: Id) -> Id);
        msg!(queue(), "addPayment:", (payment: Id) -> ());
    });
}

/// Answer the purchase in progress, if it's of `product_id`.
fn answer(product_id: &str, result: PurchaseResult) {
    let waiter = with(|s| {
        if s.purchase.as_ref().is_some_and(|(id, _)| id == product_id) {
            s.purchase.take()
        } else {
            None
        }
    });
    if let Some((_, done)) = waiter {
        done(result);
    }
}

/// `paymentQueue:updatedTransactions:`: purchases going through, failing,
/// restored, deferred.
extern "C" fn updated_transactions(_this: Id, _cmd: Sel, _queue: Id, list: Id) {
    for i in 0..count(list) {
        let tx = unsafe { msg!(list, "objectAtIndex:", (i: usize) -> Id) };
        if !tx.is_null() {
            unsafe { transaction_changed(tx) };
        }
    }
}

unsafe fn transaction_changed(tx: Id) {
    unsafe {
        let state = msg!(tx, "transactionState", () -> isize);
        let payment = msg!(tx, "payment", () -> Id);
        let product_id =
            rust_string(msg!(payment, "productIdentifier", () -> Id)).unwrap_or_default();
        match state {
            FAILED => {
                let error = msg!(tx, "error", () -> Id);
                let result = if cancelled(error) {
                    PurchaseResult::Cancelled
                } else {
                    PurchaseResult::Failed(store_error(error))
                };
                // A failed transaction is the app's to finish too.
                msg!(queue(), "finishTransaction:", (tx: Id) -> ());
                answer(&product_id, result);
            }
            PURCHASED | RESTORED | DEFERRED => {
                let state = match state {
                    PURCHASED => TransactionState::Purchased,
                    RESTORED => TransactionState::Restored,
                    _ => TransactionState::Pending,
                };
                let mut t = transaction(tx, &product_id, state);
                if state != TransactionState::Pending {
                    msg!(tx, "retain", () -> Id);
                    t.handle = with(|s| {
                        s.next += 1;
                        s.transactions.insert(s.next, tx as usize);
                        s.next
                    });
                }
                deliver(t.clone());
                match state {
                    TransactionState::Purchased => {
                        answer(&product_id, PurchaseResult::Purchased(t))
                    }
                    TransactionState::Pending => answer(&product_id, PurchaseResult::Pending),
                    _ => {}
                }
            }
            // Purchasing: the sheet is up.
            _ => {}
        }
    }
}

/// An `SKPaymentTransaction` as a [`Transaction`].
unsafe fn transaction(tx: Id, product_id: &str, state: TransactionState) -> Transaction {
    unsafe {
        let id = rust_string(msg!(tx, "transactionIdentifier", () -> Id)).unwrap_or_default();
        let original = msg!(tx, "originalTransaction", () -> Id);
        let original_id = if original.is_null() {
            id.clone()
        } else {
            rust_string(msg!(original, "transactionIdentifier", () -> Id)).unwrap_or(id.clone())
        };
        let payment = msg!(tx, "payment", () -> Id);
        let quantity = msg!(payment, "quantity", () -> isize).max(1) as u32;
        let date = msg!(tx, "transactionDate", () -> Id);
        let time = if date.is_null() {
            0
        } else {
            (msg!(date, "timeIntervalSince1970", () -> f64) * 1000.0) as u64
        };
        let mut t = Transaction::new(
            product_id,
            kind_of(product_id).unwrap_or(ProductKind::NonConsumable),
            id.clone(),
        );
        t.state = state;
        t.original_id = original_id;
        t.quantity = quantity;
        t.purchase_time_ms = time;
        t.token = id;
        t.receipt = receipt();
        t.store = StoreKind::AppStore;
        t
    }
}

/// The app receipt (`appStoreReceiptURL`), base64; empty when there's none
/// yet (a sandbox build before its first purchase).
fn receipt() -> String {
    with_pool(|| unsafe {
        let bundle = msg!(class("NSBundle"), "mainBundle", () -> Id);
        let url = msg!(bundle, "appStoreReceiptURL", () -> Id);
        if url.is_null() {
            return String::new();
        }
        let data = msg!(class("NSData"), "dataWithContentsOfURL:", (url: Id) -> Id);
        if data.is_null() {
            return String::new();
        }
        let options = 0usize;
        rust_string(msg!(data, "base64EncodedStringWithOptions:", (options: usize) -> Id))
            .unwrap_or_default()
    })
}

/// Whether StoreKit's `error` is the user cancelling.
fn cancelled(error: Id) -> bool {
    !error.is_null()
        && unsafe {
            rust_string(msg!(error, "domain", () -> Id)).as_deref() == Some("SKErrorDomain")
                && msg!(error, "code", () -> isize) == SK_PAYMENT_CANCELLED
        }
}

/// An `NSError` from StoreKit as a [`StoreError`].
fn store_error(error: Id) -> StoreError {
    if error.is_null() {
        return StoreError::Store("unknown error".into());
    }
    unsafe {
        let domain = rust_string(msg!(error, "domain", () -> Id)).unwrap_or_default();
        let code = msg!(error, "code", () -> isize);
        let text = rust_string(msg!(error, "localizedDescription", () -> Id)).unwrap_or_default();
        if domain == "SKErrorDomain" {
            match code {
                SK_PAYMENT_NOT_ALLOWED => return StoreError::NotAllowed,
                SK_PRODUCT_NOT_AVAILABLE => return StoreError::UnknownProduct(String::new()),
                SK_NETWORK_FAILED => return StoreError::Network,
                _ => {}
            }
        }
        if domain == "NSURLErrorDomain" {
            return StoreError::Network;
        }
        StoreError::Store(text)
    }
}

/// `paymentQueue:restoreCompletedTransactionsFailedWithError:`.
extern "C" fn restore_failed(_this: Id, _cmd: Sel, _queue: Id, error: Id) {
    eprintln!(
        "stipple: restoring purchases failed: {}",
        store_error(error)
    );
}

pub(crate) fn finish(t: &Transaction) {
    let Some(tx) = with(|s| s.transactions.remove(&t.handle)) else {
        return;
    };
    let tx = tx as Id;
    with_pool(|| unsafe {
        msg!(queue(), "finishTransaction:", (tx: Id) -> ());
        msg!(tx, "release", () -> ());
    });
}

pub(crate) fn restore() {
    observe();
    with_pool(|| unsafe { msg!(queue(), "restoreCompletedTransactions", () -> ()) });
}
