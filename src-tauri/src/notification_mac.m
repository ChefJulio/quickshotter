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

// Returns NSScreen.mainScreen.backingScaleFactor (2.0 on Retina, 1.0 on standard).
double qs_get_backing_scale_factor(void) {
    NSScreen *screen = [NSScreen mainScreen];
    if (!screen) return 2.0; // safe default for Retina Macs
    return (double)[screen backingScaleFactor];
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
