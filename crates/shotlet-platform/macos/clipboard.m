#import <AppKit/AppKit.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

// Pasteboard calls belong on the main thread. A worker that is already the
// main thread must not dispatch_sync back to itself.
static int on_main(int (^body)(void)) {
    if ([NSThread isMainThread]) return body();
    __block int code = 2;
    dispatch_sync(dispatch_get_main_queue(), ^{
        code = body();
    });
    return code;
}

static int write_png(const unsigned char *bytes, size_t len) {
    @autoreleasepool {
        if (!bytes || len == 0) return 1;
        NSPasteboard *board = [NSPasteboard generalPasteboard];
        if (!board) return 2;
        [board clearContents];
        NSData *data = [NSData dataWithBytes:bytes length:len];
        [board declareTypes:@[NSPasteboardTypePNG] owner:nil];
        return [board setData:data forType:NSPasteboardTypePNG] ? 0 : 3;
    }
}

int shotlet_copy_png(const unsigned char *bytes, size_t len) {
    return on_main(^{
        return write_png(bytes, len);
    });
}

int shotlet_read_png(unsigned char **out, size_t *len) {
    if (!out || !len) return 1;
    return on_main(^{
        @autoreleasepool {
            NSData *data = [[NSPasteboard generalPasteboard] dataForType:NSPasteboardTypePNG];
            if (!data || data.length == 0) return 1;
            unsigned char *buf = malloc(data.length);
            if (!buf) return 2;
            memcpy(buf, data.bytes, data.length);
            *out = buf;
            *len = data.length;
            return 0;
        }
    });
}

void shotlet_free(void *ptr) {
    free(ptr);
}
