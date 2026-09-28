#import <AppKit/AppKit.h>
#import <CoreServices/CoreServices.h>
#import <objc/runtime.h>

static void (*return_to_menu_bar)(void);

int horizon_allows_system_quit(uint32_t reason) {
    return reason == kAEShutDown || reason == kAERestart ||
        reason == kAEReallyLogOut || reason == kAELogOut;
}

static NSApplicationTerminateReply horizon_should_terminate(
    __unused id delegate, __unused SEL selector, __unused NSApplication *app
) {
    NSAppleEventDescriptor *event = NSAppleEventManager.sharedAppleEventManager.currentAppleEvent;
    // Dock Quit sends the native terminate: action, not a Tauri ExitRequested.
    // Session-ending Apple events must still be accepted so this utility can
    // never veto shutdown, restart or logout. Force Quit is unaffected.
    uint32_t reason = [[event paramDescriptorForKeyword:kAEQuitReason] enumCodeValue];
    if (horizon_allows_system_quit(reason)) {
        return NSTerminateNow;
    }
    if (return_to_menu_bar != NULL) {
        return_to_menu_bar();
    }
    return NSTerminateCancel;
}

int horizon_install_termination_guard(void (*on_return_to_menu_bar)(void)) {
    if (!NSThread.isMainThread || NSApp.delegate == nil) {
        return 0;
    }
    // Add the optional delegate method to Tao's existing delegate; never
    // replace that delegate or overwrite an upstream implementation.
    Class delegate_class = object_getClass(NSApp.delegate);
    SEL selector = @selector(applicationShouldTerminate:);
    if (class_getInstanceMethod(delegate_class, selector) != NULL) {
        return 0;
    }
    return_to_menu_bar = on_return_to_menu_bar;
    return class_addMethod(delegate_class, selector, (IMP)horizon_should_terminate, "Q@:@") ? 1 : 0;
}
