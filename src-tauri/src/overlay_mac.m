#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <objc/runtime.h>
#include <stdio.h>
#include <string.h>

// Real ObjC implementation. Its IMP is copied onto the overlay subclass so the
// frame is not pulled back onto the screen that created the window. The type
// encoding comes from NSWindow's own method, not a handwritten string.
@interface NSWindow (ShotletOverlayFrame)
- (NSRect)shotlet_unconstrainedFrameRect:(NSRect)frameRect toScreen:(NSScreen *)screen;
@end

@implementation NSWindow (ShotletOverlayFrame)
- (NSRect)shotlet_unconstrainedFrameRect:(NSRect)frameRect toScreen:(NSScreen *)screen {
    (void)screen;
    return frameRect;
}
@end

static void shotlet_bypass_constrain(NSWindow *window) {
    Class current = object_getClass(window);
    if (current == Nil) {
        return;
    }
    const char *base = class_getName(current);
    if (base != NULL && strstr(base, "_ShotletOverlay") != NULL) {
        return;
    }
    char name[256];
    snprintf(name, sizeof(name), "%s_ShotletOverlay", base != NULL ? base : "NSWindow");
    Class subclass = objc_getClass(name);
    if (subclass == Nil) {
        subclass = objc_allocateClassPair(current, name, 0);
        if (subclass == Nil) {
            return;
        }
        Method original = class_getInstanceMethod(current, @selector(constrainFrameRect:toScreen:));
        Method source = class_getInstanceMethod([NSWindow class], @selector(shotlet_unconstrainedFrameRect:toScreen:));
        const char *types = original != NULL ? method_getTypeEncoding(original) : NULL;
        IMP imp = source != NULL ? method_getImplementation(source) : NULL;
        if (types == NULL || imp == NULL || !class_addMethod(subclass, @selector(constrainFrameRect:toScreen:), imp, types)) {
            objc_disposeClassPair(subclass);
            return;
        }
        objc_registerClassPair(subclass);
    }
    object_setClass(window, subclass);
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
        shotlet_apply_level(window);
        window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary;
        window.hidesOnDeactivate = NO;
        window.movable = NO;
        window.ignoresMouseEvents = NO;
        [window setAcceptsMouseMovedEvents:YES];
        [window setFrame:frame display:YES animate:NO];
        shotlet_fit_content(window);
        shotlet_apply_level(window);
        if (make_key) {
            [NSApp activateIgnoringOtherApps:YES];
            [window makeKeyAndOrderFront:nil];
        } else {
            [window orderFrontRegardless];
        }
        shotlet_apply_level(window);
        NSRect actual = window.frame;
        if (fabs(actual.origin.x - frame.origin.x) > 1.5 || fabs(actual.origin.y - frame.origin.y) > 1.5
            || fabs(actual.size.width - frame.size.width) > 1.5 || fabs(actual.size.height - frame.size.height) > 1.5) {
            NSLog(@"shotlet overlay frame mismatch wanted %@ got %@", NSStringFromRect(frame), NSStringFromRect(actual));
        }
        // tao's always-on-top change is queued and can drop this back to floating
        // after we return. Re-apply once that queue has drained.
        __weak NSWindow *weakWindow = window;
        dispatch_async(dispatch_get_main_queue(), ^{
            NSWindow *held = weakWindow;
            if (held != nil && held.isVisible) {
                shotlet_apply_level(held);
            }
        });
    };
    if ([NSThread isMainThread]) {
        place();
    } else {
        dispatch_sync(dispatch_get_main_queue(), place);
    }
    return matched;
}
