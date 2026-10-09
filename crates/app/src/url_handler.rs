//! Receive `postgres://…`-style links that macOS routes to the app (registered through
//! `CFBundleURLTypes` in Info.plist). winit does not surface the "get URL" Apple Event, so the
//! app installs its own handler.
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;

/// `kInternetEventClass` and `kAEGetURL` are both the four-char code 'GURL'.
const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
/// `keyDirectObject`, the event parameter that carries the URL string: '----'.
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

extern "C-unwind" fn handle_get_url(
    _this: &AnyObject,
    _sel: Sel,
    event: &AnyObject,
    _reply: &AnyObject,
) {
    let descriptor: *mut AnyObject =
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let Some(descriptor) = (unsafe { descriptor.as_ref() }) else {
        return;
    };
    let url: *mut NSString = unsafe { msg_send![descriptor, stringValue] };
    if let Some(url) = unsafe { url.as_ref() } {
        ui::open_connection_url(url.to_string());
    }
}

/// Register the handler. Must run on the main thread before the event loop starts: when a
/// link launches the app, macOS delivers it during launch, before the first window exists.
pub fn install() {
    let class = AnyClass::get(c"PlusplusUrlHandler").unwrap_or_else(|| {
        let mut builder =
            ClassBuilder::new(c"PlusplusUrlHandler", AnyClass::get(c"NSObject").unwrap()).unwrap();
        unsafe {
            builder.add_method(
                sel!(handleGetURLEvent:withReplyEvent:),
                handle_get_url as extern "C-unwind" fn(_, _, _, _),
            );
        }
        builder.register()
    });
    let Some(manager_class) = AnyClass::get(c"NSAppleEventManager") else {
        return;
    };
    let handler: Retained<AnyObject> = unsafe { msg_send![class, new] };
    let manager: *mut AnyObject = unsafe { msg_send![manager_class, sharedAppleEventManager] };
    let Some(manager) = (unsafe { manager.as_ref() }) else {
        return;
    };
    unsafe {
        let _: () = msg_send![
            manager,
            setEventHandler: &*handler,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: GET_URL,
            andEventID: GET_URL
        ];
    }
    // The event manager does not retain its handler; it lives for the whole process.
    std::mem::forget(handler);
}
