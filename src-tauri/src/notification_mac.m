#import <UserNotifications/UserNotifications.h>

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
