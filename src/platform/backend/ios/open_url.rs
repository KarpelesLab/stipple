//! URLs that open the app on iOS, for
//! [`App::on_open_url`](crate::App::on_open_url): the custom schemes its
//! `Info.plist` claims (`CFBundleURLTypes`) and universal links, the https
//! links of the domains its `com.apple.developer.associated-domains`
//! entitlement names (an `NSUserActivity` of type
//! `NSUserActivityTypeBrowsingWeb`).
//!
//! Under the scene lifecycle (cargo-stipple's `Info.plist`), UIKit hands them
//! to the scene delegate: `scene:openURLContexts:` and
//! `scene:continueUserActivity:` while it runs, and the
//! `UISceneConnectionOptions` of `scene:willConnectToSession:options:` when
//! the URL launched the app (those two methods aren't called then). Without
//! scenes, the app delegate's `application:openURL:options:` and
//! `application:continueUserActivity:restorationHandler:` hear both; a launch
//! by URL also names it in the launch options, and the same URL then comes
//! again through `application:openURL:options:`, delivered once.

use std::ffi::c_void;

use super::{Class, Id, Sel, class_addMethod, msg_id, objc_msgSend, sel, set_objects, string_of};
use crate::platform::incoming_url;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationLaunchOptionsURLKey: Id;
}

/// `NSUserActivityTypeBrowsingWeb`'s value: a universal link (or Handoff
/// from a browser).
const BROWSING_WEB: &str = "NSUserActivityTypeBrowsingWeb";

/// An `NSURL`'s `absoluteString` (empty for `nil`).
unsafe fn url_string(url: Id) -> String {
    if url.is_null() {
        return String::new();
    }
    unsafe { string_of(msg_id(url, sel("absoluteString"))) }
}

/// A universal link's URL: an `NSUserActivity` browsing the web carries it
/// as its `webpageURL`.
unsafe fn activity_url(activity: Id) -> Option<String> {
    if activity.is_null() {
        return None;
    }
    unsafe {
        if string_of(msg_id(activity, sel("activityType"))) != BROWSING_WEB {
            return None;
        }
        let url = url_string(msg_id(activity, sel("webpageURL")));
        (!url.is_empty()).then_some(url)
    }
}

/// `application:openURL:options:` (no scenes): a custom scheme's URL.
extern "C" fn app_open_url(_this: Id, _cmd: Sel, _app: Id, url: Id, _options: Id) -> bool {
    incoming_url::deliver_opened(unsafe { url_string(url) });
    true
}

/// `application:continueUserActivity:restorationHandler:` (no scenes): a
/// universal link.
extern "C" fn app_continue(_this: Id, _cmd: Sel, _app: Id, activity: Id, _handler: Id) -> bool {
    match unsafe { activity_url(activity) } {
        Some(url) => {
            incoming_url::deliver(url);
            true
        }
        None => false,
    }
}

/// `scene:openURLContexts:`: a set of `UIOpenURLContext`s, each a URL.
extern "C" fn scene_open_urls(_this: Id, _cmd: Sel, _scene: Id, contexts: Id) {
    unsafe { deliver_contexts(contexts) }
}

/// `scene:continueUserActivity:`: a universal link, while the scene runs.
extern "C" fn scene_continue(_this: Id, _cmd: Sel, _scene: Id, activity: Id) {
    if let Some(url) = unsafe { activity_url(activity) } {
        incoming_url::deliver(url);
    }
}

unsafe fn deliver_contexts(contexts: Id) {
    unsafe {
        for context in set_objects(contexts) {
            incoming_url::deliver(url_string(msg_id(context, sel("URL"))));
        }
    }
}

/// From `application:didFinishLaunchingWithOptions:`: the URL the launch
/// options name (without scenes), delivered now, so it's queued for the
/// app's first events however UIKit follows up.
///
/// # Safety
/// `options` must be the launch options dictionary, or `nil`.
pub(super) unsafe fn launched(options: Id) {
    if options.is_null() {
        return;
    }
    unsafe {
        let object_for_key: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let url = url_string(object_for_key(
            options,
            sel("objectForKey:"),
            UIApplicationLaunchOptionsURLKey,
        ));
        if !url.is_empty() {
            incoming_url::deliver_launch(url);
        }
    }
}

/// From `scene:willConnectToSession:options:`: the URLs and universal links
/// that launched the app (or connected this scene), which UIKit hands over
/// only here.
///
/// # Safety
/// `options` must be the `UISceneConnectionOptions`, or `nil`.
pub(super) unsafe fn scene_connected(options: Id) {
    if options.is_null() {
        return;
    }
    unsafe {
        deliver_contexts(msg_id(options, sel("URLContexts")));
        for activity in set_objects(msg_id(options, sel("userActivities"))) {
            if let Some(url) = activity_url(activity) {
                incoming_url::deliver(url);
            }
        }
    }
}

/// Give the app delegate `application:openURL:options:` and
/// `application:continueUserActivity:restorationHandler:`.
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_app_delegate_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("application:openURL:options:"),
            app_open_url as *const c_void,
            c"B@:@@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("application:continueUserActivity:restorationHandler:"),
            app_continue as *const c_void,
            c"B@:@@@?".as_ptr(),
        );
    }
}

/// Give the scene delegate `scene:openURLContexts:` and
/// `scene:continueUserActivity:`.
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_scene_delegate_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("scene:openURLContexts:"),
            scene_open_urls as *const c_void,
            c"v@:@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("scene:continueUserActivity:"),
            scene_continue as *const c_void,
            c"v@:@@".as_ptr(),
        );
    }
}
