//! In-app purchases on Android, for [`platform::store`](crate::platform::store):
//! the Google Play Billing Library, driven over JNI.
//!
//! Google Play's billing service is only spoken through its Java library
//! (`com.android.billingclient:billing`, shipped as an AAR): the AIDL
//! interface apps once bound to directly is closed to them (Google Play
//! rejects apps that don't use the library), so there's no talking to the
//! service from Rust alone. The library must be in the APK: a Gradle build
//! adds it as a dependency; a Gradle-less one dexes it with
//! `tools/android/billing.py` (as `examples/androiddemo/build-apk.sh` does
//! with `STIPPLE_BILLING=1`). Stipple adds no Java of its own: the library's
//! classes are looked up in the app's class loader at run time, its builders
//! called by name, and its listener interfaces implemented by
//! `java.lang.reflect.Proxy` objects whose invocation handler is the native
//! [`INVOKE`](crate::platform::dex::INVOKE) class ([`invoke`] here). So the
//! code doesn't depend on the library's version: it's written for 6 to 9 (8 changed
//! the product listener's arguments; both shapes are read) and run against 8.
//!
//! Without the library (or without Google Play on the device, or signed out
//! of it), every call answers [`StoreError::Unavailable`] with the reason.
//!
//! Everything touching Java runs on the main thread (the library calls its
//! listeners there too); the state between calls is in [`BILLING`].

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use super::jni::{Env, JObject, JValue, Jni, in_native_object};
use super::{classes, log, run_on_main};
use crate::platform::store::{
    Product, ProductId, ProductKind, ProductsFn, PurchaseFn, PurchaseResult, StoreError, StoreKind,
    Transaction, TransactionState, deliver, kind_of,
};

pub(crate) const KIND: StoreKind = StoreKind::GooglePlay;

const API: &str = "com.android.billingclient.api.";

// `BillingClient.BillingResponseCode`.
const OK: i32 = 0;
const USER_CANCELED: i32 = 1;
const BILLING_UNAVAILABLE: i32 = 3;
const ITEM_UNAVAILABLE: i32 = 4;
const ITEM_ALREADY_OWNED: i32 = 7;
const NETWORK_ERROR: i32 = 12;
const SERVICE_UNAVAILABLE: i32 = 2;
const SERVICE_DISCONNECTED: i32 = -1;
const FEATURE_NOT_SUPPORTED: i32 = -2;
const SERVICE_TIMEOUT: i32 = -3;

// `Purchase.PurchaseState`.
const PURCHASED: i32 = 1;
const PENDING: i32 = 2;

/// A value taken once by whichever of two paths gets to it first.
type Once<T> = Arc<Mutex<Option<T>>>;

fn once<T>(v: T) -> Once<T> {
    Arc::new(Mutex::new(Some(v)))
}

fn take<T>(o: &Once<T>) -> Option<T> {
    o.lock().unwrap_or_else(|e| e.into_inner()).take()
}

type JobFn = dyn FnOnce(&Jni, JObject) + Send;

/// Work waiting for the connection to Google Play: run with the main thread's
/// env and the activity once connected, or failed.
struct Job {
    run: Box<JobFn>,
    fail: Box<dyn FnOnce(StoreError) + Send>,
}

#[derive(Clone, Debug, PartialEq)]
enum Conn {
    Idle,
    Connecting,
    Ready,
}

/// What a listener proxy answers for.
enum Call {
    /// The connection's `BillingClientStateListener`.
    Setup,
    /// A product query, one of the queries of `join`, for one product type.
    Products(Arc<Mutex<Join>>),
    /// The user's purchases of a type (`queryPurchasesAsync`): unfinished
    /// ones at launch, or all of them to restore.
    Owned { restore: bool, subs: bool },
    /// A consume or an acknowledgement, answered only in the log.
    Done(&'static str),
}

/// The product queries of one [`products`] call (one per product type),
/// joined.
struct Join {
    kinds: HashMap<String, ProductKind>,
    left: usize,
    found: Vec<Product>,
    error: Option<StoreError>,
    done: Option<ProductsFn>,
}

/// The library's objects (global references, as addresses) and what's
/// waiting on them.
struct Billing {
    /// The activity's class loader, which has the library's classes.
    loader: usize,
    /// An instance of the native invocation handler.
    handler: usize,
    client: usize,
    conn: Conn,
    waiting: Vec<Job>,
    /// Listener proxies waiting for their call, and what they answer.
    calls: Vec<(usize, Call)>,
    /// `ProductDetails` by product id: what a purchase is launched with.
    details: HashMap<String, usize>,
    /// The purchase in progress: its product and its answer.
    purchase: Option<(String, PurchaseFn)>,
    /// Purchases handed to the app, by handle: token, acknowledged.
    tokens: HashMap<u64, (String, bool)>,
    next: u64,
    /// Whether the purchases left unfinished were asked for (once a run).
    swept: bool,
}

// SAFETY: the addresses are global references, used on the main thread.
static BILLING: Mutex<Option<Billing>> = Mutex::new(None);

/// The reason when the library couldn't be set up, for every later call.
static MISSING: Mutex<Option<StoreError>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Billing) -> R) -> Option<R> {
    BILLING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

// ---- Setting up --------------------------------------------------------------

/// Load class `name` (binary, `com.x.Y$Z`) through `loader`.
fn load(jni: &Jni, loader: JObject, name: &str) -> Option<JObject> {
    let jname = jni.string(name)?;
    jni.call_object(
        loader,
        c"loadClass",
        c"(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue { l: jname }],
    )
}

/// A library class.
fn api(jni: &Jni, name: &str) -> Option<JObject> {
    let loader = with(|b| b.loader)? as JObject;
    load(jni, loader, &format!("{API}{name}"))
}

/// A library builder (`X.Builder`) being filled in.
struct Builder<'a> {
    jni: &'a Jni,
    obj: JObject,
    /// The JNI type its setters return: `Lcom/…/X$Builder;`.
    ret: String,
}

impl<'a> Builder<'a> {
    /// `builder.name(arg)`, `arg` of JNI type `arg_type`.
    fn set(self, name: &CStr, arg_type: &str, arg: JValue) -> Option<Self> {
        let sig = std::ffi::CString::new(format!("({arg_type}){}", self.ret)).ok()?;
        let obj = self.jni.call_object(self.obj, name, &sig, &[arg])?;
        Some(Self { obj, ..self })
    }

    /// `builder.name()`: a flag setter.
    fn flag(self, name: &CStr) -> Option<Self> {
        let sig = std::ffi::CString::new(format!("(){}", self.ret)).ok()?;
        let obj = self.jni.call_object(self.obj, name, &sig, &[])?;
        Some(Self { obj, ..self })
    }

    /// `builder.build()`, of library class `type_name`.
    fn build(self, type_name: &str) -> Option<JObject> {
        let sig = std::ffi::CString::new(format!("()Lcom/android/billingclient/api/{type_name};"))
            .ok()?;
        self.jni.call_object(self.obj, c"build", &sig, &[])
    }
}

/// `X.newBuilder(args)` of library class `name` (`X` or `X$Inner`), whose
/// parameters are `params` (JNI types).
fn builder_with<'a>(
    jni: &'a Jni,
    name: &str,
    params: &str,
    args: &[JValue],
) -> Option<Builder<'a>> {
    let class = api(jni, name)?;
    let ret = format!("Lcom/android/billingclient/api/{name}$Builder;");
    let sig = std::ffi::CString::new(format!("({params}){ret}")).ok()?;
    let obj = jni.call_static_object_on(class, c"newBuilder", &sig, args)?;
    Some(Builder { jni, obj, ret })
}

/// `X.newBuilder()` of library class `name`.
fn builder<'a>(jni: &'a Jni, name: &str) -> Option<Builder<'a>> {
    builder_with(jni, name, "", &[])
}

/// A proxy implementing library interface `iface`, answered by [`invoke`].
fn proxy(jni: &Jni, iface: &str) -> Option<JObject> {
    let (loader, handler) = with(|b| (b.loader, b.handler))?;
    let iface = load(jni, loader as JObject, &format!("{API}{iface}"))?;
    let class_class = jni.find_class(c"java/lang/Class")?;
    let ifaces = jni.object_array(class_class, &[iface])?;
    jni.call_static_object(
        c"java/lang/reflect/Proxy",
        c"newProxyInstance",
        c"(Ljava/lang/ClassLoader;[Ljava/lang/Class;Ljava/lang/reflect/InvocationHandler;)Ljava/lang/Object;",
        &[
            JValue { l: loader as JObject },
            JValue { l: ifaces },
            JValue {
                l: handler as JObject,
            },
        ],
    )
}

/// A proxy for `iface`, kept until its call comes, answering `call`.
fn listener(jni: &Jni, iface: &str, call: Call) -> Option<JObject> {
    let p = proxy(jni, iface)?;
    let g = jni.global(p) as usize;
    with(|b| b.calls.push((g, call)))?;
    Some(p)
}

/// Make the `BillingClient`, once a process. `Err` if the library isn't in
/// the APK.
fn init(jni: &Jni, activity: JObject) -> Result<(), StoreError> {
    if BILLING.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Ok(());
    }
    if let Some(e) = MISSING.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Err(e);
    }
    let missing = |why: &str| {
        let e = StoreError::Unavailable(why.into());
        log(&format!("stipple: billing: {why}"));
        *MISSING.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.clone());
        Err(e)
    };
    let Some(loader) = jni.call_object(
        activity,
        c"getClassLoader",
        c"()Ljava/lang/ClassLoader;",
        &[],
    ) else {
        return missing("no class loader");
    };
    if load(jni, loader, &format!("{API}BillingClient")).is_none() {
        return missing(
            "the Play Billing Library isn't in the APK (see docs/purchases.md: add \
             com.android.billingclient:billing, or tools/android/billing.py)",
        );
    }
    let Some(handler) = classes::invoke_class().and_then(|c| jni.new_object(c, c"()V", &[])) else {
        return missing("the invocation handler class didn't load");
    };
    *BILLING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Billing {
        loader: jni.global(loader) as usize,
        handler: jni.global(handler) as usize,
        client: 0,
        conn: Conn::Idle,
        waiting: Vec::new(),
        calls: Vec::new(),
        details: HashMap::new(),
        purchase: None,
        tokens: HashMap::new(),
        next: 0,
        swept: false,
    });
    match make_client(jni, activity) {
        Some(client) => {
            with(|b| b.client = jni.global(client) as usize);
            log("stipple: billing: client made");
            Ok(())
        }
        None => {
            jni.failed("making the BillingClient");
            *BILLING.lock().unwrap_or_else(|e| e.into_inner()) = None;
            missing("the Play Billing Library didn't make a client (an unsupported version?)")
        }
    }
}

/// `BillingClient.newBuilder(app context).setListener(…).enablePendingPurchases(…).build()`.
fn make_client(jni: &Jni, activity: JObject) -> Option<JObject> {
    let context = jni.call_object(
        activity,
        c"getApplicationContext",
        c"()Landroid/content/Context;",
        &[],
    )?;
    let b = builder_with(
        jni,
        "BillingClient",
        "Landroid/content/Context;",
        &[JValue { l: context }],
    )?;
    // The listener for every purchase update: it stays for the process.
    let updates = proxy(jni, "PurchasesUpdatedListener")?;
    jni.global(updates);
    let b = b.set(
        c"setListener",
        "Lcom/android/billingclient/api/PurchasesUpdatedListener;",
        JValue { l: updates },
    )?;
    // Pending purchases: `enablePendingPurchases(PendingPurchasesParams)`
    // from library 7 (the only form in 8), `enablePendingPurchases()` before.
    let pending = builder(jni, "PendingPurchasesParams")
        .and_then(|pb| pb.flag(c"enableOneTimeProducts"))
        .and_then(|pb| pb.build("PendingPurchasesParams"));
    let b = match pending {
        Some(params) => b.set(
            c"enablePendingPurchases",
            "Lcom/android/billingclient/api/PendingPurchasesParams;",
            JValue { l: params },
        )?,
        None => b.flag(c"enablePendingPurchases")?,
    };
    // Library 8 reconnects by itself when asked to; older ones don't have it.
    let (obj, ret) = (b.obj, b.ret.clone());
    let b = b
        .flag(c"enableAutoServiceReconnection")
        .unwrap_or(Builder { jni, obj, ret });
    b.build("BillingClient")
}

/// Run `job` once connected to Google Play: connect first if need be.
fn connected(job: Job) {
    let job = once(job);
    let j = job.clone();
    let ok = run_on_main(move |jni, activity| {
        if let Some(job) = take(&j) {
            connect(jni, activity, job);
        }
    });
    if !ok && let Some(job) = take(&job) {
        (job.fail)(StoreError::Unavailable("the app has no activity".into()));
    }
}

/// On the main thread: run `job` now if connected, else after connecting.
fn connect(jni: &Jni, activity: JObject, job: Job) {
    if let Err(e) = init(jni, activity) {
        (job.fail)(e);
        return;
    }
    let conn = with(|b| b.conn.clone());
    match conn {
        Some(Conn::Ready) => {
            if jni.push_frame() {
                (job.run)(jni, activity);
                jni.clear();
                jni.pop_frame();
            }
        }
        Some(Conn::Connecting) => {
            with(|b| b.waiting.push(job));
        }
        _ => {
            with(|b| {
                b.waiting.push(job);
                b.conn = Conn::Connecting;
            });
            let client = with(|b| b.client).unwrap_or(0) as JObject;
            let started = listener(jni, "BillingClientStateListener", Call::Setup).and_then(|l| {
                jni.call_void(
                    client,
                    c"startConnection",
                    c"(Lcom/android/billingclient/api/BillingClientStateListener;)V",
                    &[JValue { l }],
                )
            });
            if started.is_none() {
                jni.failed("startConnection");
                setup_finished(
                    jni,
                    Err(StoreError::Unavailable(
                        "Google Play billing didn't start".into(),
                    )),
                );
            }
        }
    }
}

/// The connection is up, or failed: run or fail what waited for it.
fn setup_finished(jni: &Jni, result: Result<(), StoreError>) {
    let waiting = with(|b| {
        b.conn = if result.is_ok() {
            Conn::Ready
        } else {
            Conn::Idle
        };
        std::mem::take(&mut b.waiting)
    })
    .unwrap_or_default();
    match result {
        Ok(()) => {
            log("stipple: billing: connected to Google Play");
            // The purchases left unfinished by an earlier run (or approved
            // since), once a run.
            if with(|b| !std::mem::replace(&mut b.swept, true)).unwrap_or(false) {
                query_owned(jni, false);
            }
            for job in waiting {
                // With the activity, which a listener call doesn't have.
                let job = once(job);
                let j = job.clone();
                let ok = run_on_main(move |jni, activity| {
                    if let Some(job) = take(&j) {
                        connect(jni, activity, job);
                    }
                });
                if !ok && let Some(job) = take(&job) {
                    (job.fail)(StoreError::Unavailable("the app has no activity".into()));
                }
            }
        }
        Err(e) => {
            log(&format!("stipple: billing: can't connect: {e}"));
            for job in waiting {
                (job.fail)(e.clone());
            }
        }
    }
}

// ---- The listeners -----------------------------------------------------------

/// `InvocationHandler.invoke`: a call on one of the listener proxies (or
/// `Object`'s `hashCode`, `equals`, `toString`, which a proxy forwards too).
pub(super) unsafe extern "C" fn invoke(
    env: Env,
    _this: JObject,
    proxy: JObject,
    method: JObject,
    args: JObject,
) -> JObject {
    unsafe {
        in_native_object(env, |jni| {
            let name = jni.call_object(method, c"getName", c"()Ljava/lang/String;", &[])?;
            let name = jni.to_rust(name)?;
            let args = jni.object_vec(args);
            match name.as_str() {
                "hashCode" => {
                    let h = jni.call_static_int(
                        c"java/lang/System",
                        c"identityHashCode",
                        c"(Ljava/lang/Object;)I",
                        &[JValue { l: proxy }],
                    )?;
                    jni.call_static_object(
                        c"java/lang/Integer",
                        c"valueOf",
                        c"(I)Ljava/lang/Integer;",
                        &[JValue { i: h }],
                    )
                }
                "equals" => {
                    let same = args.first().is_some_and(|&o| jni.same(proxy, o));
                    jni.call_static_object(
                        c"java/lang/Boolean",
                        c"valueOf",
                        c"(Z)Ljava/lang/Boolean;",
                        &[JValue { z: same as u8 }],
                    )
                }
                "toString" => jni.string("stipple billing listener"),
                _ => {
                    heard(jni, proxy, &name, &args);
                    None
                }
            }
        })
    }
}

/// A `BillingResult`'s code and debug message.
fn result_of(jni: &Jni, result: JObject) -> (i32, String) {
    if result.is_null() {
        return (OK, String::new());
    }
    let code = jni
        .call_int(result, c"getResponseCode", c"()I", &[])
        .unwrap_or(-100);
    let message = jni
        .call_object(result, c"getDebugMessage", c"()Ljava/lang/String;", &[])
        .and_then(|s| jni.to_rust16(s))
        .unwrap_or_default();
    (code, message)
}

/// The store error for a response code.
fn error_for(code: i32, message: &str, product: &str) -> StoreError {
    match code {
        BILLING_UNAVAILABLE | FEATURE_NOT_SUPPORTED => StoreError::Unavailable(format!(
            "Google Play billing is unavailable ({code}{}{message})",
            if message.is_empty() { "" } else { ": " }
        )),
        ITEM_UNAVAILABLE => StoreError::UnknownProduct(product.to_string()),
        ITEM_ALREADY_OWNED => StoreError::AlreadyOwned,
        NETWORK_ERROR | SERVICE_UNAVAILABLE | SERVICE_DISCONNECTED | SERVICE_TIMEOUT => {
            StoreError::Network
        }
        _ => StoreError::Store(format!("Google Play: {code} {message}")),
    }
}

/// The call that `proxy` answers, taken out (and the proxy let go).
fn call_for(jni: &Jni, proxy: JObject) -> Option<Call> {
    // Under the lock: the library answers on threads of its own, two queries
    // at once. (`IsSameObject` doesn't call into Java.)
    let (g, call) = with(|b| {
        let at = b
            .calls
            .iter()
            .position(|(g, _)| jni.same(*g as JObject, proxy))?;
        Some(b.calls.remove(at))
    })??;
    jni.delete_global(g as JObject);
    Some(call)
}

fn heard(jni: &Jni, proxy: JObject, method: &str, args: &[JObject]) {
    let arg = |i: usize| args.get(i).copied().unwrap_or(std::ptr::null_mut());
    match method {
        "onPurchasesUpdated" => {
            let (code, message) = result_of(jni, arg(0));
            purchases_updated(jni, code, &message, arg(1));
        }
        "onBillingServiceDisconnected" => {
            log("stipple: billing: disconnected from Google Play");
            with(|b| b.conn = Conn::Idle);
        }
        "onBillingSetupFinished" => {
            call_for(jni, proxy);
            let (code, message) = result_of(jni, arg(0));
            log(&format!(
                "stipple: billing: setup finished: {code} {message}"
            ));
            let result = if code == OK {
                Ok(())
            } else {
                Err(error_for(code, &message, ""))
            };
            setup_finished(jni, result);
        }
        _ => {
            let Some(call) = call_for(jni, proxy) else {
                return;
            };
            let (code, message) = result_of(jni, arg(0));
            match call {
                Call::Products(join) => product_details(jni, &join, code, &message, arg(1)),
                Call::Owned { restore, subs } => {
                    if code == OK {
                        for p in list(jni, arg(1)) {
                            purchase_seen(jni, p, Seen::Owned { restore, subs });
                        }
                    } else {
                        log(&format!(
                            "stipple: billing: purchases query: {code} {message}"
                        ));
                        if restore {
                            log("stipple: billing: restoring purchases failed");
                        }
                    }
                }
                Call::Done(what) => {
                    if code != OK {
                        log(&format!(
                            "stipple: billing: {what} failed: {code} {message}"
                        ));
                    }
                }
                Call::Setup => {}
            }
        }
    }
}

/// The elements of a `java.util.List` (none for `null`).
fn list(jni: &Jni, l: JObject) -> Vec<JObject> {
    if l.is_null() {
        return Vec::new();
    }
    let n = jni.call_int(l, c"size", c"()I", &[]).unwrap_or(0);
    (0..n)
        .filter_map(|i| jni.call_object(l, c"get", c"(I)Ljava/lang/Object;", &[JValue { i }]))
        .collect()
}

fn string_of(jni: &Jni, obj: JObject, getter: &CStr) -> String {
    if obj.is_null() {
        return String::new();
    }
    jni.call_object(obj, getter, c"()Ljava/lang/String;", &[])
        .and_then(|s| jni.to_rust16(s))
        .unwrap_or_default()
}

// ---- Products ----------------------------------------------------------------

pub(crate) fn products(ids: Vec<ProductId>, done: ProductsFn) {
    if ids.is_empty() {
        done(Ok(Vec::new()));
        return;
    }
    let done = once(done);
    let d = done.clone();
    connected(Job {
        run: Box::new(move |jni, _activity| {
            let Some(done) = take(&d) else { return };
            query_products(jni, &ids, done);
        }),
        fail: Box::new(move |e| {
            if let Some(done) = take(&done) {
                done(Err(e));
            }
        }),
    });
}

/// Ask for `ids`: one query for the one-time products, one for the
/// subscriptions (Google Play takes one type a query).
fn query_products(jni: &Jni, ids: &[ProductId], done: ProductsFn) {
    let (subs, inapp): (Vec<&ProductId>, Vec<&ProductId>) = ids
        .iter()
        .partition(|i| i.kind == ProductKind::Subscription);
    let groups: Vec<(&str, Vec<&ProductId>)> = [("inapp", inapp), ("subs", subs)]
        .into_iter()
        .filter(|(_, g)| !g.is_empty())
        .collect();
    let join = Arc::new(Mutex::new(Join {
        kinds: ids.iter().map(|i| (i.id.clone(), i.kind)).collect(),
        left: groups.len(),
        found: Vec::new(),
        error: None,
        done: Some(done),
    }));
    for (kind, group) in groups {
        if query_group(jni, kind, &group, &join).is_none() {
            jni.failed("queryProductDetailsAsync");
            finish_query(
                &join,
                Err(StoreError::Store("the product query couldn't start".into())),
            );
        }
    }
}

fn query_group(jni: &Jni, kind: &str, group: &[&ProductId], join: &Arc<Mutex<Join>>) -> Option<()> {
    let array = jni.new_named(c"java/util/ArrayList", c"()V", &[])?;
    let jkind = jni.string(kind)?;
    for id in group {
        let b = builder(jni, "QueryProductDetailsParams$Product")?;
        let jid = jni.string(&id.id)?;
        let b = b.set(c"setProductId", "Ljava/lang/String;", JValue { l: jid })?;
        let b = b.set(c"setProductType", "Ljava/lang/String;", JValue { l: jkind })?;
        let product = b.build("QueryProductDetailsParams$Product")?;
        jni.call_bool(
            array,
            c"add",
            c"(Ljava/lang/Object;)Z",
            &[JValue { l: product }],
        )?;
    }
    let b = builder(jni, "QueryProductDetailsParams")?;
    let b = b.set(c"setProductList", "Ljava/util/List;", JValue { l: array })?;
    let params = b.build("QueryProductDetailsParams")?;
    let l = listener(
        jni,
        "ProductDetailsResponseListener",
        Call::Products(join.clone()),
    )?;
    let client = with(|b| b.client)? as JObject;
    jni.call_void(
        client,
        c"queryProductDetailsAsync",
        c"(Lcom/android/billingclient/api/QueryProductDetailsParams;Lcom/android/billingclient/api/ProductDetailsResponseListener;)V",
        &[JValue { l: params }, JValue { l }],
    )
}

/// One query of `join` ended.
fn finish_query(join: &Arc<Mutex<Join>>, result: Result<Vec<Product>, StoreError>) {
    let done = {
        let mut j = join.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(mut found) => j.found.append(&mut found),
            Err(e) => j.error = Some(e),
        }
        j.left = j.left.saturating_sub(1);
        if j.left > 0 {
            return;
        }
        let out = match j.error.take() {
            // Some found: the failure was the other type's; report what was.
            Some(e) if j.found.is_empty() => Err(e),
            _ => Ok(std::mem::take(&mut j.found)),
        };
        j.done.take().map(|d| (d, out))
    };
    if let Some((done, out)) = done {
        done(out);
    }
}

/// `onProductDetailsResponse(BillingResult, List<ProductDetails>)` (library
/// 5 to 7) or `(BillingResult, QueryProductDetailsResult)` (8).
fn product_details(jni: &Jni, join: &Arc<Mutex<Join>>, code: i32, message: &str, arg: JObject) {
    if code != OK {
        finish_query(join, Err(error_for(code, message, "")));
        return;
    }
    let items = match jni.call_object(arg, c"getProductDetailsList", c"()Ljava/util/List;", &[]) {
        Some(l) => l,
        None => arg,
    };
    let kinds = join.lock().unwrap_or_else(|e| e.into_inner()).kinds.clone();
    let mut found = Vec::new();
    for d in list(jni, items) {
        let id = string_of(jni, d, c"getProductId");
        let kind = kinds
            .get(&id)
            .copied()
            .unwrap_or(ProductKind::NonConsumable);
        if let Some(p) = product(jni, d, id.clone(), kind) {
            let g = jni.global(d) as usize;
            if let Some(Some(old)) = with(|b| b.details.insert(id, g)) {
                jni.delete_global(old as JObject);
            }
            found.push(p);
        }
    }
    finish_query(join, Ok(found));
}

/// A `ProductDetails` as a [`Product`].
fn product(jni: &Jni, d: JObject, id: String, kind: ProductKind) -> Option<Product> {
    // `getName` is the bare name; `getTitle` adds the app's name.
    let mut title = string_of(jni, d, c"getName");
    if title.is_empty() {
        title = string_of(jni, d, c"getTitle");
    }
    let mut p =
        Product::new(id, kind)
            .title(title)
            .description(string_of(jni, d, c"getDescription"));
    // A one-time product's price, or a subscription's base plan's recurring
    // phase (the last).
    let price = if kind == ProductKind::Subscription {
        let offers = jni.call_object(
            d,
            c"getSubscriptionOfferDetails",
            c"()Ljava/util/List;",
            &[],
        );
        let offers = list(jni, offers.unwrap_or(std::ptr::null_mut()));
        let offer = base_offer(jni, &offers)?;
        let phases = jni.call_object(
            offer,
            c"getPricingPhases",
            c"()Lcom/android/billingclient/api/ProductDetails$PricingPhases;",
            &[],
        )?;
        let phases = jni.call_object(phases, c"getPricingPhaseList", c"()Ljava/util/List;", &[])?;
        let phase = *list(jni, phases).last()?;
        p = p.period(string_of(jni, phase, c"getBillingPeriod"));
        phase
    } else {
        jni.call_object(
            d,
            c"getOneTimePurchaseOfferDetails",
            c"()Lcom/android/billingclient/api/ProductDetails$OneTimePurchaseOfferDetails;",
            &[],
        )?
    };
    let micros = jni
        .call_long(price, c"getPriceAmountMicros", c"()J", &[])
        .unwrap_or(0);
    Some(p.price(
        string_of(jni, price, c"getFormattedPrice"),
        micros,
        string_of(jni, price, c"getPriceCurrencyCode"),
    ))
}

/// A subscription's base plan (the offer without an offer id), else its
/// first offer.
fn base_offer(jni: &Jni, offers: &[JObject]) -> Option<JObject> {
    offers
        .iter()
        .copied()
        .find(|&o| {
            jni.call_object(o, c"getOfferId", c"()Ljava/lang/String;", &[])
                .is_none()
        })
        .or_else(|| offers.first().copied())
}

// ---- Purchases ---------------------------------------------------------------

pub(crate) fn purchase(product_id: String, done: PurchaseFn) {
    let done = once(done);
    let d = done.clone();
    connected(Job {
        run: Box::new(move |jni, activity| {
            let Some(done) = take(&d) else { return };
            launch(jni, activity, product_id, done);
        }),
        fail: Box::new(move |e| {
            if let Some(done) = take(&done) {
                done(PurchaseResult::Failed(e));
            }
        }),
    });
}

/// Open the purchase sheet for `product_id`, asking for its details first
/// if they aren't known.
fn launch(jni: &Jni, activity: JObject, product_id: String, done: PurchaseFn) {
    if with(|b| b.purchase.is_some()).unwrap_or(false) {
        done(PurchaseResult::Failed(StoreError::Busy));
        return;
    }
    let details = with(|b| b.details.get(&product_id).copied()).flatten();
    let Some(details) = details else {
        // Not asked about yet: ask (as the kind the app gave, or a one-time
        // product, then a subscription), then buy.
        let kinds = match kind_of(&product_id) {
            Some(k) => vec![k],
            None => vec![ProductKind::NonConsumable, ProductKind::Subscription],
        };
        ask_then_buy(product_id, kinds, done);
        return;
    };
    let id = product_id.clone();
    with(|b| b.purchase = Some((product_id, done)));
    if start_flow(jni, activity, details as JObject).is_none() {
        jni.failed("launchBillingFlow");
        answer(
            &id,
            PurchaseResult::Failed(StoreError::Store("the purchase sheet didn't open".into())),
        );
    }
}

fn ask_then_buy(product_id: String, mut kinds: Vec<ProductKind>, done: PurchaseFn) {
    let kind = kinds.remove(0);
    let id = product_id.clone();
    products(
        vec![ProductId::new(product_id, kind)],
        Box::new(move |r| match r {
            Ok(found) if found.iter().any(|p| p.id == id) => purchase(id, done),
            Ok(_) if !kinds.is_empty() => ask_then_buy(id, kinds, done),
            Ok(_) => done(PurchaseResult::Failed(StoreError::UnknownProduct(id))),
            Err(e) => done(PurchaseResult::Failed(e)),
        }),
    );
}

/// `launchBillingFlow(activity, BillingFlowParams)` for `details`; the
/// outcome arrives at `onPurchasesUpdated`.
fn start_flow(jni: &Jni, activity: JObject, details: JObject) -> Option<()> {
    let b = builder(jni, "BillingFlowParams$ProductDetailsParams")?;
    let mut b = b.set(
        c"setProductDetails",
        "Lcom/android/billingclient/api/ProductDetails;",
        JValue { l: details },
    )?;
    // A subscription is bought through one of its offers: the base plan.
    let offers = jni
        .call_object(
            details,
            c"getSubscriptionOfferDetails",
            c"()Ljava/util/List;",
            &[],
        )
        .map(|l| list(jni, l))
        .unwrap_or_default();
    if let Some(offer) = base_offer(jni, &offers) {
        let token = jni.call_object(offer, c"getOfferToken", c"()Ljava/lang/String;", &[])?;
        b = b.set(c"setOfferToken", "Ljava/lang/String;", JValue { l: token })?;
    }
    let item = b.build("BillingFlowParams$ProductDetailsParams")?;
    let array = jni.new_named(c"java/util/ArrayList", c"()V", &[])?;
    jni.call_bool(
        array,
        c"add",
        c"(Ljava/lang/Object;)Z",
        &[JValue { l: item }],
    )?;
    let b = builder(jni, "BillingFlowParams")?;
    let b = b.set(
        c"setProductDetailsParamsList",
        "Ljava/util/List;",
        JValue { l: array },
    )?;
    let params = b.build("BillingFlowParams")?;
    let client = with(|b| b.client)? as JObject;
    let result = jni.call_object(
        client,
        c"launchBillingFlow",
        c"(Landroid/app/Activity;Lcom/android/billingclient/api/BillingFlowParams;)Lcom/android/billingclient/api/BillingResult;",
        &[JValue { l: activity }, JValue { l: params }],
    )?;
    let (code, message) = result_of(jni, result);
    if code != OK {
        log(&format!(
            "stipple: billing: launchBillingFlow: {code} {message}"
        ));
        let id = with(|b| b.purchase.as_ref().map(|p| p.0.clone()))
            .flatten()
            .unwrap_or_default();
        let r = if code == USER_CANCELED {
            PurchaseResult::Cancelled
        } else {
            PurchaseResult::Failed(error_for(code, &message, &id))
        };
        answer_any(r);
    }
    Some(())
}

/// Answer the purchase in progress if it's of `product_id`.
fn answer(product_id: &str, result: PurchaseResult) {
    let waiter = with(|b| {
        if b.purchase.as_ref().is_some_and(|(id, _)| id == product_id) {
            b.purchase.take()
        } else {
            None
        }
    })
    .flatten();
    if let Some((_, done)) = waiter {
        done(result);
    }
}

/// Answer the purchase in progress, whatever it's of.
fn answer_any(result: PurchaseResult) {
    if let Some(Some((_, done))) = with(|b| b.purchase.take()) {
        done(result);
    }
}

/// `onPurchasesUpdated`: the purchase sheet's outcome (or a purchase made
/// elsewhere: a pending one completing, a promo code redeemed).
fn purchases_updated(jni: &Jni, code: i32, message: &str, purchases: JObject) {
    match code {
        OK => {
            for p in list(jni, purchases) {
                purchase_seen(jni, p, Seen::Updated);
            }
        }
        USER_CANCELED => answer_any(PurchaseResult::Cancelled),
        ITEM_ALREADY_OWNED => {
            answer_any(PurchaseResult::Failed(StoreError::AlreadyOwned));
            // Hand it to the app again, if it never finished it.
            query_owned(jni, false);
        }
        _ => {
            let id = with(|b| b.purchase.as_ref().map(|p| p.0.clone()))
                .flatten()
                .unwrap_or_default();
            answer_any(PurchaseResult::Failed(error_for(code, message, &id)));
        }
    }
}

/// Where a purchase was heard.
#[derive(Clone, Copy)]
enum Seen {
    /// `onPurchasesUpdated`.
    Updated,
    /// A `queryPurchasesAsync` of one-time products or subscriptions: to
    /// restore everything, or to hand back what wasn't finished.
    Owned { restore: bool, subs: bool },
}

/// A `Purchase` heard: hand it to the app.
fn purchase_seen(jni: &Jni, p: JObject, seen: Seen) {
    let state = jni
        .call_int(p, c"getPurchaseState", c"()I", &[])
        .unwrap_or(0);
    let acknowledged = jni
        .call_bool(p, c"isAcknowledged", c"()Z", &[])
        .unwrap_or(false);
    let products = jni
        .call_object(p, c"getProducts", c"()Ljava/util/List;", &[])
        .map(|l| list(jni, l))
        .unwrap_or_default();
    let product_id = products
        .first()
        .and_then(|&s| jni.to_rust16(s))
        .unwrap_or_default();
    let restore = matches!(seen, Seen::Owned { restore: true, .. });
    let state = match state {
        PURCHASED if restore => TransactionState::Restored,
        PURCHASED => {
            // Granted and finished on an earlier run: nothing to hand back.
            if acknowledged && matches!(seen, Seen::Owned { .. }) {
                return;
            }
            TransactionState::Purchased
        }
        PENDING => TransactionState::Pending,
        _ => return,
    };
    let subs = matches!(seen, Seen::Owned { subs: true, .. });
    let kind = kind_of(&product_id).unwrap_or(if subs {
        ProductKind::Subscription
    } else {
        ProductKind::NonConsumable
    });
    let token = string_of(jni, p, c"getPurchaseToken");
    let order = string_of(jni, p, c"getOrderId");
    let id = if order.is_empty() {
        token.clone()
    } else {
        order
    };
    let mut t = Transaction::new(product_id.clone(), kind, id);
    t.state = state;
    t.quantity = jni
        .call_int(p, c"getQuantity", c"()I", &[])
        .unwrap_or(1)
        .max(1) as u32;
    t.purchase_time_ms = jni
        .call_long(p, c"getPurchaseTime", c"()J", &[])
        .unwrap_or(0)
        .max(0) as u64;
    t.receipt = string_of(jni, p, c"getOriginalJson");
    t.signature = string_of(jni, p, c"getSignature");
    t.token = token.clone();
    t.store = StoreKind::GooglePlay;
    if state != TransactionState::Pending {
        t.handle = with(|b| {
            b.next += 1;
            b.tokens.insert(b.next, (token, acknowledged));
            b.next
        })
        .unwrap_or(0);
    }
    deliver(t.clone());
    if let Seen::Updated = seen {
        match state {
            TransactionState::Purchased => answer(&product_id, PurchaseResult::Purchased(t)),
            TransactionState::Pending => answer(&product_id, PurchaseResult::Pending),
            _ => {}
        }
    }
}

/// Ask for the user's purchases, one-time products and subscriptions.
fn query_owned(jni: &Jni, restore: bool) {
    for subs in [false, true] {
        let started = (|| {
            let b = builder(jni, "QueryPurchasesParams")?;
            let kind = jni.string(if subs { "subs" } else { "inapp" })?;
            let b = b.set(c"setProductType", "Ljava/lang/String;", JValue { l: kind })?;
            let params = b.build("QueryPurchasesParams")?;
            let l = listener(
                jni,
                "PurchasesResponseListener",
                Call::Owned { restore, subs },
            )?;
            let client = with(|b| b.client)? as JObject;
            jni.call_void(
                client,
                c"queryPurchasesAsync",
                c"(Lcom/android/billingclient/api/QueryPurchasesParams;Lcom/android/billingclient/api/PurchasesResponseListener;)V",
                &[JValue { l: params }, JValue { l }],
            )
        })();
        if started.is_none() {
            jni.failed("queryPurchasesAsync");
        }
    }
}

pub(crate) fn finish(t: &Transaction) {
    let Some(Some((token, acknowledged))) = with(|b| b.tokens.remove(&t.handle)) else {
        return;
    };
    // By the transaction's kind, which the app may have corrected.
    let consume = t.kind == ProductKind::Consumable;
    if !consume && acknowledged {
        return;
    }
    connected(Job {
        run: Box::new(move |jni, _activity| {
            let started = (|| {
                let jtoken = jni.string(&token)?;
                let client = with(|b| b.client)? as JObject;
                if consume {
                    let b = builder(jni, "ConsumeParams")?;
                    let b = b.set(
                        c"setPurchaseToken",
                        "Ljava/lang/String;",
                        JValue { l: jtoken },
                    )?;
                    let params = b.build("ConsumeParams")?;
                    let l = listener(jni, "ConsumeResponseListener", Call::Done("consuming"))?;
                    jni.call_void(
                        client,
                        c"consumeAsync",
                        c"(Lcom/android/billingclient/api/ConsumeParams;Lcom/android/billingclient/api/ConsumeResponseListener;)V",
                        &[JValue { l: params }, JValue { l }],
                    )
                } else {
                    let b = builder(jni, "AcknowledgePurchaseParams")?;
                    let b = b.set(
                        c"setPurchaseToken",
                        "Ljava/lang/String;",
                        JValue { l: jtoken },
                    )?;
                    let params = b.build("AcknowledgePurchaseParams")?;
                    let l = listener(
                        jni,
                        "AcknowledgePurchaseResponseListener",
                        Call::Done("acknowledging"),
                    )?;
                    jni.call_void(
                        client,
                        c"acknowledgePurchase",
                        c"(Lcom/android/billingclient/api/AcknowledgePurchaseParams;Lcom/android/billingclient/api/AcknowledgePurchaseResponseListener;)V",
                        &[JValue { l: params }, JValue { l }],
                    )
                }
            })();
            if started.is_none() {
                jni.failed("finishing a purchase");
            }
        }),
        fail: Box::new(|e| log(&format!("stipple: billing: can't finish a purchase: {e}"))),
    });
}

pub(crate) fn restore() {
    connected(Job {
        run: Box::new(|jni, _activity| query_owned(jni, true)),
        fail: Box::new(|e| log(&format!("stipple: billing: can't restore purchases: {e}"))),
    });
}

pub(crate) fn start() {
    connected(Job {
        run: Box::new(|_, _| {}),
        fail: Box::new(|_| {}),
    });
}
