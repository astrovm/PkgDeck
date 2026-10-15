// Native end-to-end test. Drive the real menu bar and close button through
// Accessibility, using a disposable bundle and settings supplied by the runner.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

static NSRunningApplication *running;

static id attribute(AXUIElementRef element, CFStringRef name) {
    CFTypeRef value = NULL;
    if (!element || AXUIElementCopyAttributeValue(element, name, &value) != kAXErrorSuccess) return nil;
    return CFBridgingRelease(value);
}

static void require(BOOL condition, NSString *message) {
    if (condition) return;
    fprintf(stderr, "FAIL: %s\n", message.UTF8String);
    [running forceTerminate];
    exit(1);
}

static BOOL until(BOOL (^condition)(void)) {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:15];
    do {
        if (condition()) return YES;
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    } while (deadline.timeIntervalSinceNow > 0);
    return NO;
}

static NSArray *children(AXUIElementRef element) {
    return attribute(element, kAXChildrenAttribute) ?: @[];
}

static AXUIElementRef menu_entry(AXUIElementRef app, NSString *title) {
    id bar = attribute(app, CFSTR("AXExtrasMenuBar"));
    for (id icon in children((__bridge AXUIElementRef)bar)) {
        for (id menu in children((__bridge AXUIElementRef)icon)) {
            for (id entry in children((__bridge AXUIElementRef)menu)) {
                if ([attribute((__bridge AXUIElementRef)entry, kAXTitleAttribute) isEqual:title]) {
                    return (AXUIElementRef)CFRetain((__bridge AXUIElementRef)entry);
                }
            }
        }
    }
    return NULL;
}

static void press_menu(AXUIElementRef app, NSString *title) {
    AXUIElementRef entry = menu_entry(app, title);
    require(entry != NULL, [@"Menu item missing: " stringByAppendingString:title]);
    require([attribute(entry, kAXEnabledAttribute) boolValue], [@"Menu item disabled: " stringByAppendingString:title]);
    AXError error = AXUIElementPerformAction(entry, kAXPressAction);
    CFRelease(entry);
    require(error == kAXErrorSuccess, [@"Could not press: " stringByAppendingString:title]);
}

static NSArray *windows(AXUIElementRef app) {
    return attribute(app, kAXWindowsAttribute) ?: @[];
}

static void check_menu(AXUIElementRef app) {
    for (NSString *title in @[@"Open", @"Check now", @"Quit"]) {
        AXUIElementRef entry = menu_entry(app, title);
        require(entry != NULL, [@"Menu item missing: " stringByAppendingString:title]);
        require([attribute(entry, kAXEnabledAttribute) boolValue], [@"Menu item disabled: " stringByAppendingString:title]);
        CFRelease(entry);
    }
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        require(argc == 3, @"Usage: macos-lifecycle /path/Test.app /path/settings-root");
        if (!AXIsProcessTrusted()) {
            fprintf(stderr, "SKIP: native lifecycle test requires Accessibility access\n");
            return 77;
        }
        NSString *root = [NSString stringWithUTF8String:argv[2]];
        for (NSNumber *background in @[@YES, @NO]) {
            NSWorkspaceOpenConfiguration *config = [NSWorkspaceOpenConfiguration configuration];
            config.activates = NO;
            config.createsNewApplicationInstance = YES;
            config.arguments = background.boolValue ? @[@"--background"] : @[];
            config.environment = @{
                @"XDG_CONFIG_HOME": [root stringByAppendingPathComponent:@"config"],
                @"XDG_DATA_HOME": [root stringByAppendingPathComponent:@"data"],
                @"XDG_CACHE_HOME": [root stringByAppendingPathComponent:@"cache"],
                @"XDG_RUNTIME_DIR": root,
            };
            __block BOOL launched = NO;
            [[NSWorkspace sharedWorkspace] openApplicationAtURL:[NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]]
                configuration:config completionHandler:^(NSRunningApplication *app, NSError *error) {
                    require(error == nil, [NSString stringWithFormat:@"Launch failed: %@", error]);
                    running = app;
                    launched = YES;
                }];
            require(until(^BOOL { return launched; }), @"Launch timed out");
            pid_t pid = running.processIdentifier;
            AXUIElementRef app = AXUIElementCreateApplication(pid);
            require(until(^BOOL {
                AXUIElementRef entry = menu_entry(app, @"Open");
                if (entry) CFRelease(entry);
                return entry != NULL;
            }), @"Menu bar icon never registered");
            check_menu(app);
            require(running.finishedLaunching, @"AppKit never finished launching");
            if (background.boolValue) {
                require(windows(app).count == 0, @"Background startup showed a window");
                require(running.activationPolicy == NSApplicationActivationPolicyAccessory, @"Background startup showed a Dock icon");
                press_menu(app, @"Open");
            }
            for (int cycle = 0; cycle < 3; cycle++) {
                require(until(^BOOL { return windows(app).count == 1; }), @"Open did not create one window");
                require(running.activationPolicy == NSApplicationActivationPolicyRegular, @"Window has no Dock icon");
                id window = windows(app).firstObject;
                id close = attribute((__bridge AXUIElementRef)window, kAXCloseButtonAttribute);
                require(close != nil, @"Window has no accessible close button");
                require(AXUIElementPerformAction((__bridge AXUIElementRef)close, kAXPressAction) == kAXErrorSuccess, @"Close button failed");
                require(until(^BOOL { return windows(app).count == 0; }), @"Close left a window open");
                // The old implementation exec'ed after two idle seconds.
                [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:3]];
                require(!running.terminated && running.processIdentifier == pid, @"Closing replaced or terminated the app");
                require(running.activationPolicy == NSApplicationActivationPolicyAccessory, @"Closed window left a Dock icon");
                check_menu(app);
                if (cycle == 0) {
                    press_menu(app, @"Check now");
                    require(windows(app).count == 0, @"Check now unexpectedly showed a window");
                }
                press_menu(app, @"Open");
            }
            require(until(^BOOL { return windows(app).count == 1; }), @"Final reopen failed");
            press_menu(app, @"Quit");
            require(until(^BOOL { return running.terminated; }), @"Quit did not terminate the app");
            CFRelease(app);
            printf("PASS: %s startup, three close/reopen cycles, background Check now, Quit\n", background.boolValue ? "background" : "normal");
        }
    }
    return 0;
}
