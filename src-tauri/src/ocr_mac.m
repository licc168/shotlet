#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static char *shotlet_dup_utf8(NSString *text) {
    if (text == nil) {
        text = @"";
    }
    const char *utf8 = text.UTF8String;
    if (utf8 == NULL) {
        utf8 = "";
    }
    size_t length = strlen(utf8);
    char *copy = malloc(length + 1);
    if (copy == NULL) {
        return NULL;
    }
    memcpy(copy, utf8, length + 1);
    return copy;
}

static void shotlet_set_error(char **error_out, NSString *message) {
    if (error_out == NULL) {
        return;
    }
    *error_out = shotlet_dup_utf8(message.length > 0 ? message : @"文字识别失败");
}

static NSString *shotlet_join_observations(NSArray<VNRecognizedTextObservation *> *observations) {
    if (observations.count == 0) {
        return @"";
    }
    NSArray<VNRecognizedTextObservation *> *sorted =
        [observations sortedArrayUsingComparator:^NSComparisonResult(VNRecognizedTextObservation *left,
                                                                      VNRecognizedTextObservation *right) {
          CGRect left_box = left.boundingBox;
          CGRect right_box = right.boundingBox;
          CGFloat left_top = CGRectGetMaxY(left_box);
          CGFloat right_top = CGRectGetMaxY(right_box);
          CGFloat tolerance = MAX(left_box.size.height, right_box.size.height) * 0.5;
          if (fabs(left_top - right_top) > tolerance) {
              return left_top > right_top ? NSOrderedAscending : NSOrderedDescending;
          }
          if (fabs(left_box.origin.x - right_box.origin.x) > 0.002) {
              return left_box.origin.x < right_box.origin.x ? NSOrderedAscending : NSOrderedDescending;
          }
          return NSOrderedSame;
        }];

    NSMutableArray<NSString *> *lines = [NSMutableArray array];
    NSMutableArray<NSString *> *current = [NSMutableArray array];
    CGFloat line_top = 0;
    CGFloat line_height = 0;
    BOOL has_line = NO;
    for (VNRecognizedTextObservation *observation in sorted) {
        VNRecognizedText *candidate = [[observation topCandidates:1] firstObject];
        NSString *text = [candidate.string stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (text.length == 0) {
            continue;
        }
        CGRect box = observation.boundingBox;
        CGFloat top = CGRectGetMaxY(box);
        CGFloat tolerance = MAX(box.size.height, line_height) * 0.6;
        if (!has_line || fabs(top - line_top) > tolerance) {
            if (current.count > 0) {
                [lines addObject:[current componentsJoinedByString:@" "]];
                [current removeAllObjects];
            }
            has_line = YES;
            line_top = top;
            line_height = box.size.height;
        }
        [current addObject:text];
    }
    if (current.count > 0) {
        [lines addObject:[current componentsJoinedByString:@" "]];
    }
    return [lines componentsJoinedByString:@"\n"];
}

char *shotlet_recognize_rgba(const uint8_t *pixels, uint32_t width, uint32_t height, uint32_t bytes_per_row, char **error_out) {
    if (error_out != NULL) {
        *error_out = NULL;
    }
    @autoreleasepool {
        if (pixels == NULL || width == 0 || height == 0) {
            return shotlet_dup_utf8(@"");
        }
        if (bytes_per_row < width * 4) {
            shotlet_set_error(error_out, @"文字识别收到了无效的图像");
            return NULL;
        }

        CGColorSpaceRef color_space = CGColorSpaceCreateDeviceRGB();
        CGDataProviderRef provider = CGDataProviderCreateWithData(
            NULL, pixels, (size_t)bytes_per_row * (size_t)height, NULL);
        CGBitmapInfo bitmap_info = (CGBitmapInfo)(kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGImageRef image = NULL;
        if (color_space != NULL && provider != NULL) {
            image = CGImageCreate(
                width,
                height,
                8,
                32,
                bytes_per_row,
                color_space,
                bitmap_info,
                provider,
                NULL,
                false,
                kCGRenderingIntentDefault);
        }
        if (color_space != NULL) {
            CGColorSpaceRelease(color_space);
        }
        if (provider != NULL) {
            CGDataProviderRelease(provider);
        }
        if (image == NULL) {
            shotlet_set_error(error_out, @"无法创建识别图像");
            return NULL;
        }

        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
        NSIndexSet *revisions = [VNRecognizeTextRequest supportedRevisions];
        if ([revisions containsIndex:VNRecognizeTextRequestRevision3]) {
            request.revision = VNRecognizeTextRequestRevision3;
        }
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;
        request.minimumTextHeight = 0;

        NSArray<NSString *> *preferred = @[@"zh-Hans", @"zh-Hant", @"en-US", @"ja-JP", @"ko-KR"];
        NSError *language_error = nil;
        NSArray<NSString *> *supported = [request supportedRecognitionLanguagesAndReturnError:&language_error];
        if (supported.count > 0) {
            NSMutableArray<NSString *> *chosen = [NSMutableArray array];
            for (NSString *language in preferred) {
                if ([supported containsObject:language]) {
                    [chosen addObject:language];
                }
            }
            if (chosen.count > 0) {
                request.recognitionLanguages = chosen;
            }
        }
        if ([request respondsToSelector:@selector(setAutomaticallyDetectsLanguage:)]) {
            request.automaticallyDetectsLanguage = YES;
        }

        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
        NSError *error = nil;
        BOOL ok = [handler performRequests:@[request] error:&error];
        CGImageRelease(image);
        if (!ok) {
            NSString *message = error.localizedDescription;
            if (message.length == 0) {
                message = @"文字识别失败";
            }
            shotlet_set_error(error_out, message);
            return NULL;
        }
        return shotlet_dup_utf8(shotlet_join_observations(request.results));
    }
}

void shotlet_ocr_free(char *ptr) {
    free(ptr);
}
