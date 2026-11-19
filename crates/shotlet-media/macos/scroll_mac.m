#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <dispatch/dispatch.h>
#include <unistd.h>

static pid_t window_pid_at(int x, int y) {
    CGWindowListOption options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    CFArrayRef list = CGWindowListCopyWindowInfo(options, kCGNullWindowID);
    if (list == NULL) {
        return 0;
    }
    pid_t self_pid = getpid();
    pid_t found = 0;
    CFIndex count = CFArrayGetCount(list);
    for (CFIndex index = 0; index < count; index++) {
        CFDictionaryRef info = CFArrayGetValueAtIndex(list, index);
        if (info == NULL) {
            continue;
        }
        CFNumberRef pid_ref = CFDictionaryGetValue(info, kCGWindowOwnerPID);
        int pid = 0;
        if (pid_ref == NULL || !CFNumberGetValue(pid_ref, kCFNumberIntType, &pid) || pid == (int)self_pid || pid <= 0) {
            continue;
        }
        CFNumberRef layer_ref = CFDictionaryGetValue(info, kCGWindowLayer);
        int layer = 0;
        if (layer_ref != NULL) {
            CFNumberGetValue(layer_ref, kCFNumberIntType, &layer);
        }
        if (layer != 0) {
            continue;
        }
        CFDictionaryRef bounds_ref = CFDictionaryGetValue(info, kCGWindowBounds);
        CGRect rect = CGRectZero;
        if (bounds_ref == NULL || !CGRectMakeWithDictionaryRepresentation(bounds_ref, &rect)) {
            continue;
        }
        if (rect.size.width < 40 || rect.size.height < 40) {
            continue;
        }
        if (x < rect.origin.x || y < rect.origin.y || x >= rect.origin.x + rect.size.width || y >= rect.origin.y + rect.size.height) {
            continue;
        }
        found = (pid_t)pid;
        break;
    }
    CFRelease(list);
    return found;
}

static void activate_pid(pid_t pid) {
    if (pid <= 0) {
        return;
    }
    NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
    if (app == nil) {
        return;
    }
    [app activateWithOptions:NSApplicationActivateIgnoringOtherApps];
}

void shotlet_focus_at(int x, int y) {
    pid_t pid = window_pid_at(x, y);
    if (pid <= 0) {
        return;
    }
    if ([NSThread isMainThread]) {
        activate_pid(pid);
        return;
    }
    dispatch_sync(dispatch_get_main_queue(), ^{
        activate_pid(pid);
    });
}
