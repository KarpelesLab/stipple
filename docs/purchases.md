# In-app purchases

`stipple::platform::store()` is the app store's purchase system, with one
shape on both phones: StoreKit on iOS, Google Play Billing on Android. It
lists products in the user's language and currency, opens the store's
purchase sheet, hands every transaction to the app, and finishes them once
the app has granted what was bought. Desktops and the web have no app store:
there it answers `StoreError::Unavailable`. A mock store stands in anywhere,
for tests, CI and the desktop build of a phone app.

<!-- live: storedemo -->

## The flow

```rust,no_run
use stipple::platform::{ProductId, PurchaseResult, store};
use stipple::prelude::*;

struct State { gems: u32, no_ads: bool, prices: Vec<String>, status: String }

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let handle = cx.handle();
    column(vec![
        label(cx, format!("{} gems", state.gems)),
        button(cx, "Buy 100 gems").on_tap(cx, move |_| {
            // The sheet's outcome, for the UI. The gems arrive at on_transaction.
            handle.store().purchase("gems_100", |s: &mut State, r| {
                s.status = match r {
                    PurchaseResult::Failed(e) => e.to_string(),
                    PurchaseResult::Pending => "Waiting for approval".into(),
                    _ => String::new(),
                };
            });
        }),
        button(cx, "Restore purchases").on_tap(cx, |_| store().restore_purchases()),
    ])
}

fn main() {
    let app = App::new(
        State { gems: 0, no_ads: false, prices: vec![], status: String::new() },
        view,
    )
    // Every transaction: bought, restored, pending, unfinished from an
    // earlier run, approved while the app was closed.
    .on_transaction(|s, t| {
        if !t.is_paid() {
            return; // pending: grant nothing yet
        }
        match t.product_id.as_str() {
            "gems_100" => s.gems += 100 * t.quantity,
            "no_ads" => s.no_ads = true,
            _ => return,
        }
        // Save the state here, then:
        store().finish(t);
    });
    // The products, as the store lists them (and their kinds, which
    // `finish` needs on Google Play).
    app.handle().store().products(
        vec![ProductId::consumable("gems_100"), ProductId::non_consumable("no_ads")],
        |s: &mut State, r| s.prices = r.unwrap_or_default().into_iter().map(|p| p.price).collect(),
    );
    app.run();
}
```

- **`Store::products(ids, done)`** answers with a `Product` per id the store
  knows: `title`, `description`, `price` (formatted by the store: show this
  one), `price_micros` and `currency`, and a subscription's `period`
  (ISO 8601, `P1M`). Ids the store doesn't know are left out, as both stores
  do. Each `ProductId` says what the product is (`consumable`,
  `non_consumable`, `subscription`): neither store's original API does, and
  Google Play needs it to finish a purchase.
- **`Store::purchase(id, done)`** opens the store's sheet. `done` gets a
  `PurchaseResult`: `Purchased(transaction)`, `Pending` (Ask to Buy, or a
  cash payment at a shop on Google Play), `Cancelled` or `Failed(error)`.
  It's for the UI; the app grants purchases in one place,
  `App::on_transaction`, which sees the same transaction.
- **`App::on_transaction(|state, &Transaction|)`** hears every transaction.
  Setting it makes the app listen from launch, so a purchase interrupted by
  a crash, or approved by a parent while the app was closed, arrives there
  at the next start. A `Transaction` has the `product_id`, its `state`
  (`Purchased`, `Restored`, `Pending`), `quantity`, the identifiers, and what
  a server checks it with (below).
- **`Store::finish(&transaction)`** once the purchase is granted and saved.
  On iOS that's `finishTransaction:`; on Google Play a consumable is
  consumed (so it can be bought again) and anything else acknowledged.
  Until then the store hands the transaction back at every launch, and
  Google Play refunds a purchase that isn't acknowledged within three days.
  Finishing goes by `Transaction::kind`: the kind the app gave the product
  in `products` (Stipple remembers it), or, for a product the app never
  named this run, a guess that loses nothing (a one-time product is
  acknowledged, never consumed). Ask for the products early, or set `kind`
  before finishing.
- **`Store::restore_purchases()`** brings back non-consumables and active
  subscriptions bought before, on this device or another, as `Restored`
  transactions. Apple requires a button for it.

`AppHandle::store()` is the same calls with callbacks that change the state.
`Store`'s own callbacks run on whichever thread the store answers on.

## Platforms

| | Store | Status |
|---|---|---|
| iOS | StoreKit, the original API (`SKProductsRequest`, `SKPaymentQueue`, a transaction observer), through the Objective-C runtime | build-checked |
| Android | Google Play Billing Library 6 to 9, over JNI, with no Java of Stipple's | run on an emulator against a stand-in for the library (CI), and against the real library up to Google Play's answer |
| macOS, Windows, Linux | none: `Unavailable` | unit-tested |
| Web | none: `Unavailable` | build-checked |
| Anywhere | the mock store | unit-tested, `storedemo` |

### iOS: StoreKit

StoreKit 2 (`Product`, `Transaction`, async/await) is Swift-only: its types
have no Objective-C face, so a Rust app can't call it without a Swift shim
in the bundle. Stipple uses the original StoreKit instead, which Apple marked
deprecated in the iOS 18 SDK but still ships, supports and accepts in
review; its transactions are the same App Store transactions StoreKit 2
sees, and their identifiers work with the App Store Server API. One object
of a class built at run time is the payment queue's observer and the
product requests' delegate; it's added when the app finishes launching (if
it has an `on_transaction` hook), so the queue hands back unfinished
transactions right away. A failed transaction is finished by Stipple; a
deferred one (Ask to Buy) is reported `Pending`.

What's been verified: it compiles for `aarch64-apple-ios` and its
Objective-C calls follow StoreKit's headers; it hasn't run (StoreKit needs a
signed app with products in App Store Connect, or Xcode's StoreKit testing).

**Setting up** (App Store Connect):

1. Accept the Paid Apps agreement and fill in tax and banking (*Business*).
2. Under the app, *Monetization → In-App Purchases* (consumables and
   non-consumables) and *Subscriptions* (a subscription group with its
   subscriptions): create each product with the id the app asks for, a
   price, and a localized name and description. A product needs a review
   screenshot before it's submitted, and is reviewed with an app version.
3. The app's bundle id must match, and its provisioning profile must allow
   In-App Purchase (on by default for App IDs).

**Testing**:

- *Sandbox*: create sandbox testers in App Store Connect (*Users and Access
  → Sandbox*), sign in with one on the device under *Settings → App Store →
  Sandbox Account*, and run a development-signed build. Purchases cost
  nothing, subscriptions renew every few minutes, and a tester's purchase
  history can be cleared.
- *TestFlight* builds buy in the sandbox too.
- *StoreKit configuration files* (`.storekit`, Xcode's local store) work with
  the original StoreKit as well, but only for an app run from an Xcode scheme
  that names the file: wrap the Stipple binary in an Xcode project to use
  them.
- Ask to Buy and interrupted purchases can be simulated in the sandbox
  (Settings → App Store → Sandbox Account → Manage).

### Android: Google Play Billing

Google Play takes payments only through its Billing Library
(`com.android.billingclient:billing`), a Java library shipped as an AAR. The
billing service's AIDL interface, which apps once bound to directly, is
closed to them: Google Play requires the library (it checks the version the
calls carry) and stops supporting old versions every year. So the library
has to be in the APK, and Stipple drives it from Rust:

- its classes are looked up in the app's class loader at run time and its
  builders called by name, so no code is compiled against a version;
- its listener interfaces (`PurchasesUpdatedListener`,
  `BillingClientStateListener` and the per-call ones) are implemented by
  `java.lang.reflect.Proxy` objects whose `InvocationHandler` is a class
  Stipple defines at run time with a native `invoke` (in the dex it writes for
  its other classes), so there's no Java to ship, and listener changes
  between versions (8 changed the product listener's arguments) are read
  both ways.

Without the library the store answers `Unavailable("the Play Billing Library
isn't in the APK …")`; on a device without Google Play, or signed out of it,
`Unavailable` with Google Play's reason.

**Packaging the library**:

- *With Gradle* (an app project around the native library):
  `implementation("com.android.billingclient:billing:8.3.0")` (or 9.x), and
  since nothing in Java references the library, keep it from R8 when
  minifying:

  ```proguard
  -keep class com.android.billingclient.api.** { *; }
  ```

- *Without Gradle* (the `aapt` + `d8` build `examples/androiddemo/build-apk.sh`
  does): `tools/android/billing.py OUT --manifest AndroidManifest.xml`
  resolves the library's dependencies from Google's Maven repository and
  Maven Central, dexes them into `OUT/classes2.dex` (and on), and writes
  `OUT/AndroidManifest.xml` with the entries the library adds (the
  `com.android.vending.BILLING` permission, the `<queries>` for the billing
  service, its proxy activities). Add the dex files next to the app's
  `classes.dex`. `STIPPLE_BILLING=1 examples/androiddemo/build-apk.sh` does
  all of it. That build doesn't compile the AARs' resources, which 8.x
  doesn't need but 9.x does: stay on 8.x without Gradle.

**Setting up** (Play Console):

1. A merchant account (*Settings → Payments profile*).
2. Upload a build that has the library (so its `BILLING` permission) to a
   testing track (internal testing is enough); products can be created only
   then.
3. *Monetize → Products → In-app products* and *Subscriptions* (each with a
   base plan): the ids the app asks for, prices, names.
4. *Settings → License testing*: add the testers' Google accounts. Their
   purchases are free (test cards), and subscriptions renew within minutes.

**Testing**: install the build from the testing track's link (or sideload
one signed with the same key, with the same version code), on a device or
emulator image with Google Play, signed in with a tester account. The old
static test ids (`android.test.purchased`) don't work with the current
library's product details. Google's *Play Billing Lab* app can change the
country and simulate some responses for a tester.

What's been verified: on an Android 13 emulator with the Play Store (not
signed in) and an APK with Billing Library 8.0.0 or 8.3.0 from
`tools/android/billing.py`, the client is built through the library's
builders, connects, and its listener (a proxy over Stipple's handler) hears
Google Play answer `BILLING_UNAVAILABLE`, which reaches the app as
`StoreError::Unavailable`. The rest of the path can't run without a Play
Console listing and a signed-in tester, so CI runs it against a stand-in for
the library (`.github/billing/android`): the real library's classes and
signatures (from `javap`), answering from memory. There the products are
read (a subscription's base plan, an emoji in a name), a purchase goes
through `launchBillingFlow` to `on_transaction` and the purchase's callback,
the unfinished purchases of an earlier run arrive at launch (finished ones
don't), and finishing consumes or acknowledges. A real purchase hasn't been
made.

### Desktops and the web

There's no app store: `products` and `purchase` answer
`StoreError::Unavailable`, `restore_purchases` and `finish` do nothing.
Selling on the web goes through a payment provider and your server, not
Stipple. A phone app's desktop build can install the mock instead.

## The mock store

```rust
use stipple::platform::{MockOutcome, MockStore, Product, ProductKind, Store};

let mock = MockStore::new()
    .product(Product::new("gems_100", ProductKind::Consumable)
        .title("Pouch of gems").price("$0.99", 990_000, "USD"))
    .product(Product::new("vip", ProductKind::Subscription)
        .title("VIP").price("$4.99", 4_990_000, "USD").period("P1M"))
    .outcome("vip", MockOutcome::Pending)  // Ask to Buy, until mock.approve("vip")
    .owned("no_ads")                       // bought before: restore brings it back
    .interrupted("gems_100");              // unfinished from "last run": arrives at launch
Store::mock(mock.clone());
```

`Store::mock` installs it as the process's store in place of the real one;
keep a clone to drive it (`approve`) and look at it (`unfinished`: delivered
and not finished, `owned_products`). It answers at once on the caller's
thread, with made-up transaction ids and tokens, and refuses to sell a
non-consumable twice (`AlreadyOwned`). With no products it makes one up for
any id (at $0.99), so `STIPPLE_STORE=mock` (an empty mock) runs any app
unchanged.

In tests, `App::run_pending` delivers the mock's transactions to
`on_transaction` along with the handle's updates:

```rust,ignore
let mut app = my_app();               // installs the mock
app.run_pending();                    // the products arrive
assert!(app.click(Role::Button, "$0.99"));
app.run_pending();                    // purchase callback + on_transaction
assert!(app.find_text("100 gems").is_some());
```

`examples/storedemo` is a store screen on the mock (on a phone, on the real
store), with such a test.

## Checking purchases on a server

A purchase the app reports could be forged on a rooted or jailbroken
device. If what it buys matters (currency in an online game, a
subscription to a service), have your server check each transaction before
granting it, and grant each one once (by its id), then finish it:

- **Google Play**: send `Transaction::token` (the purchase token) and the
  product id to your server, which asks the Google Play Developer API:
  `purchases.products.get` for one-time products, `purchases.subscriptionsv2.get`
  for subscriptions (a service account linked in the Play Console). Its
  answer has the purchase state, whether it was consumed or acknowledged,
  and the order id. The server can also consume or acknowledge it there.
  `Transaction::receipt` (the purchase's JSON) and `signature` (RSA-SHA1
  with the app's licensing key from the Play Console) can be checked
  offline, which proves the JSON came from Google Play but not that it
  wasn't refunded. Real-time developer notifications (Pub/Sub) tell the
  server about renewals, cancellations and refunds.
- **App Store**: send `Transaction::id` (the transaction identifier) to your
  server, which asks the App Store Server API's *Get Transaction Info*
  (`/inApps/v1/transactions/{id}`, with an API key from App Store Connect).
  The answer is a JWS signed by Apple: check the signature and its
  certificate chain, the bundle id and the product id. App Store Server
  Notifications V2 tell the server about renewals and refunds.
  `Transaction::receipt` is the app receipt (base64) for the older
  `verifyReceipt` endpoint, which Apple deprecated; use it only where the
  server API can't be.

Either way, the app grants what the server confirms, and finishes the
transaction after; one the server refuses can be finished without granting
it.

## Good practice

- Set `on_transaction` before `run`, and ask for the products at launch:
  transactions can arrive before the user opens the store screen.
- Show the store's `price` string as it comes. Prices differ by country, and
  the user may be charged in another currency than their locale's.
- Save the grant before finishing: a crash in between hands the
  transaction back (grant by transaction id, so twice is harmless), while
  finishing first and crashing loses the purchase.
- Don't grant `Pending` transactions; the same purchase comes back
  `Purchased`, maybe days later.
- Disable the buy button while a purchase is in progress: a second one
  answers `Failed(Busy)`.
- Offer *Restore purchases* (required by App Review when there are
  non-consumables or subscriptions).
