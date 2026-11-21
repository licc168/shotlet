#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <objc/runtime.h>
#import <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

// Per-place overlay diagnostics walk every on-screen window (kCGWindowListOptionAll)
// and fflush stderr. That runs on the capture hot path after each placement; keep it
// behind a compile-time flag so normal builds never pay for it. Set to 1 to debug.
#ifndef SHOTLET_OVERLAY_TRACE
#define SHOTLET_OVERLAY_TRACE 0
#endif

// macOS 26 aborts in NSDP_getComputedPropertyValue if an NSWindow's isa is
// swapped, and aborts again if orderOut: is added to the live KVO class.
// Keep TaoWindow's class. Only constrainFrameRect:toScreen: is overridden.
// Order the window while its frame is already on the target display. Parking
// it on the main display first binds it to that display's space, and a later
// setFrame leaves it visible to AppKit but off the other screen.
static char kShotletFreeFrame;
static char kShotletPlaceToken;
static IMP shotlet_ns_constrain = NULL;

@interface NSWindow (ShotletOverlayFrame)
- (NSRect)shotlet_constrainFrameRect:(NSRect)frameRect toScreen:(NSScreen *)screen;
@end

@implementation NSWindow (ShotletOverlayFrame)
- (NSRect)shotlet_constrainFrameRect:(NSRect)frameRect toScreen:(NSScreen *)screen {
    if (objc_getAssociatedObject(self, &kShotletFreeFrame) != nil) {
        return frameRect;
    }
    if (shotlet_ns_constrain == NULL) {
        return frameRect;
    }
    typedef NSRect (*ShotletConstrain)(id, SEL, NSRect, NSScreen *);
    return ((ShotletConstrain)shotlet_ns_constrain)(self, _cmd, frameRect, screen);
}
@end

static void shotlet_cache_constrain(void) {
    if (shotlet_ns_constrain != NULL) {
        return;
    }
    Method method = class_getInstanceMethod([NSWindow class], @selector(constrainFrameRect:toScreen:));
    if (method != NULL) {
        shotlet_ns_constrain = method_getImplementation(method);
    }
}

static BOOL shotlet_class_has_method(Class cls, SEL sel) {
    unsigned int count = 0;
    Method *methods = class_copyMethodList(cls, &count);
    BOOL found = NO;
    for (unsigned int index = 0; index < count; index++) {
        if (method_getName(methods[index]) == sel) {
            found = YES;
            break;
        }
    }
    free(methods);
    return found;
}

static void shotlet_bypass_constrain(NSWindow *window) {
    if (window == nil) {
        return;
    }
    objc_setAssociatedObject(window, &kShotletFreeFrame, @YES, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    shotlet_cache_constrain();
    Class cls = object_getClass(window);
    if (cls == Nil || cls == [NSWindow class]) {
        return;
    }
    SEL sel = @selector(constrainFrameRect:toScreen:);
    IMP ours = class_getMethodImplementation([NSWindow class], @selector(shotlet_constrainFrameRect:toScreen:));
    if (ours == NULL || shotlet_class_has_method(cls, sel)) {
        return;
    }
    Method original = class_getInstanceMethod([NSWindow class], sel);
    const char *types = original != NULL ? method_getTypeEncoding(original) : NULL;
    if (types == NULL) {
        return;
    }
    class_addMethod(cls, sel, ours, types);
}

static void shotlet_unhide_view(NSView *view) {
    if (view == nil) {
        return;
    }
    view.hidden = NO;
    for (NSView *child in view.subviews) {
        shotlet_unhide_view(child);
    }
}

static void shotlet_fit_content(NSWindow *window) {
    NSView *content = window.contentView;
    if (content == nil) {
        return;
    }
    NSView *parent = content.superview;
    if (parent != nil) {
        content.frame = parent.bounds;
    }
    content.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    for (NSView *child in content.subviews) {
        child.frame = content.bounds;
        child.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    }
    shotlet_unhide_view(content);
}

static void shotlet_apply_level(NSWindow *window) {
    window.level = NSPopUpMenuWindowLevel;
}

static NSInteger shotlet_bump_token(NSWindow *window) {
    NSInteger token = [objc_getAssociatedObject(window, &kShotletPlaceToken) integerValue] + 1;
    objc_setAssociatedObject(window, &kShotletPlaceToken, @(token), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    return token;
}

typedef int (*ShotletSLSMain)(void);
typedef int (*ShotletSLSGetBounds)(int, uint32_t, CGRect *);
typedef int (*ShotletSLSOrdered)(int, uint32_t, uint8_t *);
typedef CFArrayRef (*ShotletSLSSpaces)(int, int, CFArrayRef);
typedef CFStringRef (*ShotletSLSBestDisplay)(int, CGRect);
typedef uint64_t (*ShotletSLSCurrentSpace)(int, CFStringRef);
typedef void (*ShotletSLSAddSpaces)(int, CFArrayRef, CFArrayRef);
typedef void (*ShotletSLSMoveSpace)(int, CFArrayRef, uint64_t);
typedef int (*ShotletSLSAssignAll)(int, pid_t);

static ShotletSLSMain sls_main_fn;
static ShotletSLSGetBounds sls_bounds_fn;
static ShotletSLSOrdered sls_ordered_fn;
static ShotletSLSSpaces sls_spaces_fn;
static ShotletSLSBestDisplay sls_best_fn;
static ShotletSLSCurrentSpace sls_current_fn;
static ShotletSLSAddSpaces sls_add_spaces_fn;
static ShotletSLSMoveSpace sls_move_fn;
static ShotletSLSAssignAll sls_assign_all_fn;
static BOOL sls_ready = NO;

static void shotlet_load_sls(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        void *handle = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY);
        if (handle == NULL) {
            handle = RTLD_DEFAULT;
        }
        sls_main_fn = (ShotletSLSMain)dlsym(handle, "SLSMainConnectionID");
        sls_bounds_fn = (ShotletSLSGetBounds)dlsym(handle, "SLSGetWindowBounds");
        sls_ordered_fn = (ShotletSLSOrdered)dlsym(handle, "SLSWindowIsOrderedIn");
        sls_spaces_fn = (ShotletSLSSpaces)dlsym(handle, "SLSCopySpacesForWindows");
        sls_best_fn = (ShotletSLSBestDisplay)dlsym(handle, "SLSCopyBestManagedDisplayForRect");
        sls_current_fn = (ShotletSLSCurrentSpace)dlsym(handle, "SLSManagedDisplayGetCurrentSpace");
        sls_add_spaces_fn = (ShotletSLSAddSpaces)dlsym(handle, "SLSAddWindowsToSpaces");
        sls_move_fn = (ShotletSLSMoveSpace)dlsym(handle, "SLSMoveWindowsToManagedSpace");
        sls_assign_all_fn = (ShotletSLSAssignAll)dlsym(handle, "SLSProcessAssignToAllSpaces");
        sls_ready = sls_main_fn && sls_bounds_fn && sls_spaces_fn && sls_best_fn && sls_current_fn;
    });
}

static CFArrayRef shotlet_wid_array(uint32_t wid) {
    int32_t value = (int32_t)wid;
    CFNumberRef number = CFNumberCreate(NULL, kCFNumberSInt32Type, &value);
    if (number == NULL) {
        return NULL;
    }
    CFArrayRef array = CFArrayCreate(NULL, (const void **)&number, 1, &kCFTypeArrayCallBacks);
    CFRelease(number);
    return array;
}

static NSString *shotlet_rect_string(CGRect rect) {
    if (CGRectIsNull(rect) || CGRectIsInfinite(rect)) {
        return @"null";
    }
    return [NSString stringWithFormat:@"{{%.0f, %.0f}, {%.0f, %.0f}}", rect.origin.x, rect.origin.y, rect.size.width, rect.size.height];
}

static BOOL shotlet_copy_spaces(uint32_t wid, NSString **text, BOOL *has, uint64_t target) {
    if (has != NULL) {
        *has = NO;
    }
    shotlet_load_sls();
    if (!sls_ready || wid == 0) {
        if (text != NULL) {
            *text = @"?";
        }
        return NO;
    }
    CFArrayRef windows = shotlet_wid_array(wid);
    if (windows == NULL) {
        if (text != NULL) {
            *text = @"?";
        }
        return NO;
    }
    CFArrayRef spaces = sls_spaces_fn(sls_main_fn(), 0x7, windows);
    CFRelease(windows);
    if (spaces == NULL) {
        if (text != NULL) {
            *text = @"none";
        }
        return YES;
    }
    NSMutableString *list = [NSMutableString string];
    BOOL found = NO;
    CFIndex count = CFArrayGetCount(spaces);
    for (CFIndex index = 0; index < count; index++) {
        CFNumberRef number = CFArrayGetValueAtIndex(spaces, index);
        int64_t sid = 0;
        if (number != NULL && CFGetTypeID(number) == CFNumberGetTypeID()) {
            CFNumberGetValue(number, kCFNumberSInt64Type, &sid);
        }
        if (target != 0 && (uint64_t)sid == target) {
            found = YES;
        }
        if (list.length > 0) {
            [list appendString:@","];
        }
        [list appendFormat:@"%lld", (long long)sid];
    }
    CFRelease(spaces);
    if (text != NULL) {
        *text = list.length > 0 ? list : @"empty";
    }
    if (has != NULL) {
        *has = found;
    }
    return YES;
}

static CGRect shotlet_list_bounds(NSInteger number) {
    if (number <= 0) {
        return CGRectNull;
    }
    CFArrayRef info = CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID);
    if (info == NULL) {
        return CGRectNull;
    }
    CGRect rect = CGRectNull;
    CFIndex count = CFArrayGetCount(info);
    for (CFIndex index = 0; index < count; index++) {
        NSDictionary *item = (__bridge NSDictionary *)CFArrayGetValueAtIndex(info, index);
        if ([item[(id)kCGWindowNumber] integerValue] != number) {
            continue;
        }
        CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)item[(id)kCGWindowBounds], &rect);
        break;
    }
    CFRelease(info);
    return rect;
}

static BOOL shotlet_same_frame(NSRect a, NSRect b) {
    return fabs(a.origin.x - b.origin.x) < 1.5 && fabs(a.origin.y - b.origin.y) < 1.5
        && fabs(a.size.width - b.size.width) < 1.5 && fabs(a.size.height - b.size.height) < 1.5;
}

static BOOL shotlet_cg_onscreen(NSInteger number) {
    if (number <= 0) {
        return NO;
    }
    CFArrayRef info = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    if (info == NULL) {
        return NO;
    }
    BOOL on = NO;
    CFIndex count = CFArrayGetCount(info);
    for (CFIndex index = 0; index < count; index++) {
        NSDictionary *item = (__bridge NSDictionary *)CFArrayGetValueAtIndex(info, index);
        if ([item[(id)kCGWindowNumber] integerValue] == number) {
            on = YES;
            break;
        }
    }
    CFRelease(info);
    return on;
}

static BOOL shotlet_is_attached(NSWindow *window) {
    if (window == nil || !window.isVisible) {
        return NO;
    }
    return shotlet_cg_onscreen(window.windowNumber);
}

// An onscreen window already composited on this display. Ordering relative to
// it puts us in that display's visible space. Skip the desktop and our own
// windows; a negative layer would drag the overlay under everything else.
static NSInteger shotlet_window_on_display(NSRect frame, NSInteger selfNumber) {
    CGRect display = CGRectZero;
    BOOL found = NO;
    for (NSScreen *screen in [NSScreen screens]) {
        if (!shotlet_same_frame(screen.frame, frame)) {
            continue;
        }
        id number = screen.deviceDescription[@"NSScreenNumber"];
        if (![number respondsToSelector:@selector(unsignedIntValue)]) {
            break;
        }
        display = CGDisplayBounds((CGDirectDisplayID)[number unsignedIntValue]);
        found = YES;
        break;
    }
    if (!found || CGRectIsEmpty(display)) {
        return 0;
    }
    CFArrayRef info = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    if (info == NULL) {
        return 0;
    }
    NSInteger best = 0;
    CGFloat bestArea = 0;
    CFIndex count = CFArrayGetCount(info);
    for (CFIndex index = 0; index < count; index++) {
        NSDictionary *item = (__bridge NSDictionary *)CFArrayGetValueAtIndex(info, index);
        NSInteger number = [item[(id)kCGWindowNumber] integerValue];
        if (number <= 0 || number == selfNumber) {
            continue;
        }
        NSInteger layer = [item[(id)kCGWindowLayer] integerValue];
        if (layer < 0 || layer > NSPopUpMenuWindowLevel) {
            continue;
        }
        pid_t owner = (pid_t)[item[(id)kCGWindowOwnerPID] integerValue];
        if (owner == getpid()) {
            continue;
        }
        CGRect rect = CGRectZero;
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)item[(id)kCGWindowBounds], &rect)) {
            continue;
        }
        CGRect hit = CGRectIntersection(rect, display);
        if (CGRectIsNull(hit)) {
            continue;
        }
        CGFloat area = hit.size.width * hit.size.height;
        if (area > bestArea) {
            bestArea = area;
            best = number;
        }
    }
    CFRelease(info);
    return best;
}

static uint64_t shotlet_target_space(NSRect frame, CGRect *display_bounds) {
    if (display_bounds != NULL) {
        *display_bounds = CGRectNull;
    }
    shotlet_load_sls();
    CGDirectDisplayID display = 0;
    for (NSScreen *screen in [NSScreen screens]) {
        if (!shotlet_same_frame(screen.frame, frame)) {
            continue;
        }
        id number = screen.deviceDescription[@"NSScreenNumber"];
        if (![number respondsToSelector:@selector(unsignedIntValue)]) {
            break;
        }
        display = (CGDirectDisplayID)[number unsignedIntValue];
        break;
    }
    if (display == 0 || !sls_ready) {
        return 0;
    }
    CGRect bounds = CGDisplayBounds(display);
    if (display_bounds != NULL) {
        *display_bounds = bounds;
    }
    CFStringRef uuid = sls_best_fn(sls_main_fn(), bounds);
    if (uuid == NULL) {
        return 0;
    }
    uint64_t sid = sls_current_fn(sls_main_fn(), uuid);
    CFRelease(uuid);
    return sid;
}

static CFArrayRef shotlet_sid_array(uint64_t sid) {
    int64_t value = (int64_t)sid;
    CFNumberRef number = CFNumberCreate(NULL, kCFNumberSInt64Type, &value);
    if (number == NULL) {
        return NULL;
    }
    CFArrayRef array = CFArrayCreate(NULL, (const void **)&number, 1, &kCFTypeArrayCallBacks);
    CFRelease(number);
    return array;
}

// In-process SLSMoveWindowsToManagedSpace does not change the space (macOS 14.5+).
// Assigning the process, then adding the window to the display's current space, does.
static void shotlet_assign_all_spaces(void) {
    shotlet_load_sls();
    if (sls_assign_all_fn == NULL || sls_main_fn == NULL) {
        return;
    }
    // Once is not enough: a window ordered after the first call stays on the
    // display that created it until the pointer activates the other one.
    int result = sls_assign_all_fn(sls_main_fn(), getpid());
    static int last = 999;
    if (result != last) {
        last = result;
        fprintf(stderr, "shotlet overlay assign-all rc=%d\n", result);
        fflush(stderr);
    }
}

static void shotlet_add_space(uint32_t wid, uint64_t sid) {
    if (!sls_ready || sls_add_spaces_fn == NULL || wid == 0 || sid == 0) {
        return;
    }
    BOOL has = NO;
    if (shotlet_copy_spaces(wid, NULL, &has, sid) && has) {
        return;
    }
    CFArrayRef windows = shotlet_wid_array(wid);
    CFArrayRef spaces = shotlet_sid_array(sid);
    if (windows != NULL && spaces != NULL) {
        sls_add_spaces_fn(sls_main_fn(), windows, spaces);
    }
    if (windows != NULL) {
        CFRelease(windows);
    }
    if (spaces != NULL) {
        CFRelease(spaces);
    }
    shotlet_copy_spaces(wid, NULL, &has, sid);
    if (!has && sls_move_fn != NULL) {
        CFArrayRef again = shotlet_wid_array(wid);
        if (again != NULL) {
            sls_move_fn(sls_main_fn(), again, sid);
            CFRelease(again);
        }
        shotlet_copy_spaces(wid, NULL, &has, sid);
    }
    fprintf(stderr, "shotlet overlay add-space wid=%u target=%llu has=%d\n", wid, (unsigned long long)sid, has ? 1 : 0);
    fflush(stderr);
}

// Space id of a window the user can already see on this display. The managed-
// display query can return a different id, and adding to that id does nothing,
// so the mask stays blank until the pointer activates the display.
static uint64_t shotlet_first_space(uint32_t wid) {
    shotlet_load_sls();
    if (!sls_ready || wid == 0) {
        return 0;
    }
    CFArrayRef windows = shotlet_wid_array(wid);
    if (windows == NULL) {
        return 0;
    }
    CFArrayRef spaces = sls_spaces_fn(sls_main_fn(), 0x7, windows);
    CFRelease(windows);
    if (spaces == NULL || CFArrayGetCount(spaces) < 1) {
        if (spaces != NULL) {
            CFRelease(spaces);
        }
        return 0;
    }
    int64_t sid = 0;
    CFNumberRef number = CFArrayGetValueAtIndex(spaces, 0);
    if (number != NULL && CFGetTypeID(number) == CFNumberGetTypeID()) {
        CFNumberGetValue(number, kCFNumberSInt64Type, &sid);
    }
    CFRelease(spaces);
    return (uint64_t)sid;
}

static uint64_t shotlet_space_on_display(CGRect display, NSInteger selfNumber) {
    if (CGRectIsEmpty(display) || CGRectIsNull(display)) {
        return 0;
    }
    CFArrayRef info = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    if (info == NULL) {
        return 0;
    }
    NSInteger best = 0;
    CGFloat bestArea = 0;
    pid_t me = getpid();
    CFIndex count = CFArrayGetCount(info);
    for (CFIndex index = 0; index < count; index++) {
        NSDictionary *item = (__bridge NSDictionary *)CFArrayGetValueAtIndex(info, index);
        NSInteger number = [item[(id)kCGWindowNumber] integerValue];
        if (number <= 0 || number == selfNumber) {
            continue;
        }
        if ((pid_t)[item[(id)kCGWindowOwnerPID] integerValue] == me) {
            continue;
        }
        NSInteger layer = [item[(id)kCGWindowLayer] integerValue];
        if (layer < 0 || layer > NSPopUpMenuWindowLevel) {
            continue;
        }
        CGRect rect = CGRectZero;
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)item[(id)kCGWindowBounds], &rect)) {
            continue;
        }
        CGRect hit = CGRectIntersection(rect, display);
        if (CGRectIsNull(hit)) {
            continue;
        }
        CGFloat area = hit.size.width * hit.size.height;
        if (area > bestArea) {
            bestArea = area;
            best = number;
        }
    }
    CFRelease(info);
    if (best <= 0) {
        return 0;
    }
    return shotlet_first_space((uint32_t)best);
}

static NSWindowCollectionBehavior shotlet_behavior(void) {
    // Stationary, not Transient. Transient is hidden when the pointer moves to
    // the other display and that display's space becomes the active one.
    return NSWindowCollectionBehaviorCanJoinAllSpaces
        | NSWindowCollectionBehaviorFullScreenAuxiliary
        | NSWindowCollectionBehaviorStationary
        | NSWindowCollectionBehaviorIgnoresCycle
        | NSWindowCollectionBehaviorCanJoinAllApplications
        | NSWindowCollectionBehaviorFullScreenDisallowsTiling;
}

static void shotlet_apply_behavior(NSWindow *window) {
    window.collectionBehavior = shotlet_behavior();
    window.hidesOnDeactivate = NO;
    window.animationBehavior = NSWindowAnimationBehaviorNone;
    window.movable = NO;
    window.hasShadow = NO;
    window.ignoresMouseEvents = NO;
    [window setAcceptsMouseMovedEvents:YES];
}

// Occlusion drops the visible bit when the window server has the frame on
// another space. isVisible stays YES, which is why the other screen looked up
// while the pointer was moving and nothing was actually on it.
static BOOL shotlet_appkit_visible(NSWindow *window) {
    return (window.occlusionState & NSWindowOcclusionStateVisible) != 0;
}

static void shotlet_order_on_display(NSWindow *window, NSRect frame) {
    shotlet_apply_level(window);
    window.alphaValue = 1;
    window.hidesOnDeactivate = NO;
    NSInteger relative = shotlet_window_on_display(frame, window.windowNumber);
    if (relative > 0) {
        [window orderWindow:NSWindowAbove relativeTo:relative];
    } else {
        [window orderFrontRegardless];
    }
    shotlet_apply_level(window);
    window.hidesOnDeactivate = NO;
}

static void shotlet_publish(NSWindow *window, NSRect frame, int make_key) {
    shotlet_assign_all_spaces();
    shotlet_apply_behavior(window);
    [window setFrame:frame display:YES animate:NO];
    shotlet_fit_content(window);
    // Set the frame again after joining every space, so Spaces rebinds this
    // display instead of the one that created the window.
    [window setFrame:frame display:YES animate:NO];
    if (window.windowNumber > 0) {
        CGRect display = CGRectNull;
        uint64_t queried = shotlet_target_space(frame, &display);
        uint64_t visible = shotlet_space_on_display(display, window.windowNumber);
        uint64_t target = visible != 0 ? visible : queried;
        shotlet_add_space((uint32_t)window.windowNumber, target);
    }
    shotlet_order_on_display(window, frame);
    if (make_key) {
        if (shotlet_appkit_visible(window) || shotlet_cg_onscreen(window.windowNumber)) {
            [window makeKeyWindow];
        }
    }
    shotlet_apply_level(window);
    window.hidesOnDeactivate = NO;
}

// Pointer moved onto a display that is already showing. Do not orderOut, do not
// clear CanJoinAllSpaces, and do not migrate spaces. That path blocked the main
// thread and dropped the other screen.
static void shotlet_show_on_display(NSWindow *window, NSRect frame, int make_key) {
    if (shotlet_same_frame(window.frame, frame) && shotlet_appkit_visible(window)) {
        shotlet_apply_level(window);
        window.hidesOnDeactivate = NO;
        if ((window.collectionBehavior & NSWindowCollectionBehaviorCanJoinAllSpaces) == 0) {
            shotlet_apply_behavior(window);
        }
        if (make_key && !window.isKeyWindow) {
                [window makeKeyWindow];
        }
        return;
    }
    shotlet_publish(window, frame, make_key);
}

static void shotlet_log(NSWindow *window, const char *phase) __attribute__((unused));
static void shotlet_log(NSWindow *window, const char *phase) {
    NSString *frame = NSStringFromRect(window.frame);
    NSString *screen = window.screen.localizedName;
    fprintf(stderr,
            "shotlet overlay %s visible=%d attached=%d key=%d number=%ld frame=%s screen=%s\n",
            phase,
            window.isVisible ? 1 : 0,
            shotlet_is_attached(window) ? 1 : 0,
            window.isKeyWindow ? 1 : 0,
            (long)window.windowNumber,
            frame.UTF8String != NULL ? frame.UTF8String : "?",
            screen.UTF8String != NULL ? screen.UTF8String : "-");
    CGRect sls = CGRectNull;
    shotlet_load_sls();
    if (sls_bounds_fn != NULL && window.windowNumber > 0) {
        sls_bounds_fn(sls_main_fn(), (uint32_t)window.windowNumber, &sls);
    }
    NSString *spaces = nil;
    shotlet_copy_spaces((uint32_t)MAX(window.windowNumber, 0), &spaces, NULL, 0);
    CGRect display = CGRectNull;
    uint64_t target = shotlet_target_space(window.frame, &display);
    NSInteger parent = window.parentWindow != nil ? window.parentWindow.windowNumber : 0;
    uint8_t ordered = 0;
    int ordered_rc = -1;
    if (sls_ordered_fn != NULL && window.windowNumber > 0) {
        ordered_rc = sls_ordered_fn(sls_main_fn(), (uint32_t)window.windowNumber, &ordered);
    }
    fprintf(stderr,
            "shotlet overlay %s detail ordered=%d ordered_rc=%d spaces=%s target=%llu style=%lu behavior=%lu occlusion=%lu parent=%ld sls=%s cg=%s display=%s\n",
            phase,
            ordered_rc == 0 ? (int)ordered : -1,
            ordered_rc,
            spaces.UTF8String != NULL ? spaces.UTF8String : "-",
            (unsigned long long)target,
            (unsigned long)window.styleMask,
            (unsigned long)window.collectionBehavior,
            (unsigned long)window.occlusionState,
            (long)parent,
            shotlet_rect_string(sls).UTF8String,
            shotlet_rect_string(shotlet_list_bounds(window.windowNumber)).UTF8String,
            shotlet_rect_string(display).UTF8String);
    fflush(stderr);
}

static NSRect shotlet_frame_for_display(double cg_x, double cg_y, double width, double height, int *matched) {
    if (matched != NULL) {
        *matched = 0;
    }
    for (NSScreen *screen in [NSScreen screens]) {
        id number = screen.deviceDescription[@"NSScreenNumber"];
        if (![number respondsToSelector:@selector(unsignedIntValue)]) {
            continue;
        }
        CGDirectDisplayID display = (CGDirectDisplayID)[number unsignedIntValue];
        CGRect bounds = CGDisplayBounds(display);
        if (fabs(bounds.origin.x - cg_x) < 1.5 && fabs(bounds.origin.y - cg_y) < 1.5) {
            if (matched != NULL) {
                *matched = 1;
            }
            return screen.frame;
        }
    }

    CGFloat scale = 1;
    for (NSScreen *screen in [NSScreen screens]) {
        if (fabs(screen.frame.origin.x) < 0.5 && fabs(screen.frame.origin.y) < 0.5 && screen.backingScaleFactor >= 1) {
            scale = screen.backingScaleFactor;
            break;
        }
    }
    CGFloat points_high = ((CGFloat)CGDisplayPixelsHigh(CGMainDisplayID())) / scale;
    return NSMakeRect(cg_x, points_high - cg_y - height, width, height);
}

// CG origins are top-left, y down. The window frame is the NSScreen frame, so each overlay
// sits on its own display instead of being clamped back onto the screen that created it.
int shotlet_place_overlay(void *raw, double cg_x, double cg_y, double width, double height, int make_key) {
    if (raw == NULL || width < 1 || height < 1) {
        return 0;
    }
    NSWindow *window = (__bridge NSWindow *)raw;
    __block int matched = 0;
    void (^place)(void) = ^{
        NSRect frame = shotlet_frame_for_display(cg_x, cg_y, width, height, &matched);
        shotlet_bypass_constrain(window);
        // Already on this display: do not join spaces again. That ran on every
        // pointer move, blocked the main thread, and hid the other screen.
        if (shotlet_same_frame(window.frame, frame)
            && (shotlet_appkit_visible(window) || shotlet_cg_onscreen(window.windowNumber))) {
            shotlet_apply_level(window);
            window.hidesOnDeactivate = NO;
            if (make_key && !window.isKeyWindow && shotlet_appkit_visible(window)) {
                [window makeKeyWindow];
            }
            return;
        }
        shotlet_bump_token(window);
        shotlet_show_on_display(window, frame, make_key);
        if (!shotlet_same_frame(window.frame, frame)) {
            fprintf(stderr, "shotlet overlay frame mismatch wanted %s got %s\n",
                    NSStringFromRect(frame).UTF8String,
                    NSStringFromRect(window.frame).UTF8String);
            fflush(stderr);
        }
#if SHOTLET_OVERLAY_TRACE
        shotlet_log(window, "place");
#endif
    };
    if ([NSThread isMainThread]) {
        place();
    } else {
        dispatch_sync(dispatch_get_main_queue(), place);
    }
    return matched;
}

// Invalidates a pending re-show. The caller still orderOuts through tao.
void shotlet_cancel_overlay(void *raw) {
    if (raw == NULL) {
        return;
    }
    NSWindow *window = (__bridge NSWindow *)raw;
    void (^cancel)(void) = ^{
        shotlet_bump_token(window);
    };
    if ([NSThread isMainThread]) {
        cancel();
    } else {
        dispatch_sync(dispatch_get_main_queue(), cancel);
    }
}

int shotlet_overlay_onscreen(void *raw) {
    if (raw == NULL) {
        return 0;
    }
    NSWindow *window = (__bridge NSWindow *)raw;
    __block int on = 0;
    void (^check)(void) = ^{
        on = shotlet_cg_onscreen(window.windowNumber) ? 1 : 0;
    };
    if ([NSThread isMainThread]) {
        check();
    } else {
        dispatch_sync(dispatch_get_main_queue(), check);
    }
    return on;
}
