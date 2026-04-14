#import <UserNotifications/UserNotifications.h>
#import <AppKit/AppKit.h>

// Delegate that forces notification banners to display even when
// QuickShotter is the frontmost app (macOS suppresses them by default).
@interface QSNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation QSNotificationDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionSound);
}
@end

static QSNotificationDelegate *_delegate = nil;

// Returns the highest backingScaleFactor across all screens.
// Using mainScreen alone is unreliable for Accessory apps (no key window),
// which can cause it to return a non-Retina external display's scale factor.
double qs_get_backing_scale_factor(void) {
    NSArray<NSScreen *> *screens = [NSScreen screens];
    if (!screens || screens.count == 0) return 2.0; // safe default for Retina Macs
    double maxScale = 1.0;
    for (NSScreen *screen in screens) {
        double s = [screen backingScaleFactor];
        if (s > maxScale) maxScale = s;
    }
    return maxScale;
}

// Returns the backingScaleFactor for a specific CGDirectDisplayID.
// Falls back to the max scale if the display can't be matched to an NSScreen.
double qs_get_display_scale_factor(uint32_t displayID) {
    NSArray<NSScreen *> *screens = [NSScreen screens];
    if (!screens || screens.count == 0) return 2.0;
    for (NSScreen *screen in screens) {
        NSDictionary *desc = [screen deviceDescription];
        NSNumber *screenID = desc[@"NSScreenNumber"];
        if (screenID && [screenID unsignedIntValue] == displayID) {
            return [screen backingScaleFactor];
        }
    }
    // Fallback: return max scale
    return qs_get_backing_scale_factor();
}

// Hide app from Cmd+Tab and Dock — tray-only app.
void qs_set_activation_policy_accessory(void) {
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
}

void qs_install_notification_delegate(void) {
    @try {
        _delegate = [[QSNotificationDelegate alloc] init];
        [[UNUserNotificationCenter currentNotificationCenter] setDelegate:_delegate];
    } @catch (NSException *e) {
        // UNUserNotificationCenter requires a proper app bundle.
        // Silently skip in dev builds (unbundled binary).
        NSLog(@"[notification] delegate install skipped: %@", e.reason);
    }
}
