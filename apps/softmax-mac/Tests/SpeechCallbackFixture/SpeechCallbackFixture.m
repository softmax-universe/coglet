#import "SpeechCallbackFixture.h"
#import <objc/runtime.h>

// Registrations/counts/cancellation are MainActor-only. Background deliveries
// capture immutable copies and never touch this mutable registry.
static NSMutableArray *authorizations;
static NSMutableArray *recognitions;
static NSUInteger cancellations;

@interface FixtureTranscript : SFTranscription
@property(nonatomic, copy) NSString *fixtureText;
@end
@implementation FixtureTranscript
- (NSString *)formattedString { return self.fixtureText; }
@end
@interface FixtureResult : SFSpeechRecognitionResult
@property(nonatomic, strong) FixtureTranscript *fixtureTranscript;
@property(nonatomic) BOOL fixtureFinal;
@end
@implementation FixtureResult
- (SFTranscription *)bestTranscription { return self.fixtureTranscript; }
- (BOOL)isFinal { return self.fixtureFinal; }
@end
@interface FixtureTask : SFSpeechRecognitionTask
@end
@implementation FixtureTask
- (void)cancel { dispatch_assert_queue(dispatch_get_main_queue()); cancellations++; }
@end

// Deliberately retain the SDK's unannotated Objective-C block contract. A Swift
// fake taking a Swift closure elides the isolation thunk and MISSES the RC3 trap.
@interface SFSpeechRecognizer (CallbackFixture)
+ (void)fixture_authorize:(void (^)(SFSpeechRecognizerAuthorizationStatus))callback;
- (BOOL)fixture_supportsOnDeviceRecognition;
- (SFSpeechRecognitionTask *)fixture_recognitionTaskWithRequest:(SFSpeechRecognitionRequest *)request
    resultHandler:(void (^)(SFSpeechRecognitionResult *, NSError *))callback;
@end
@implementation SFSpeechRecognizer (CallbackFixture)
+ (void)fixture_authorize:(void (^)(SFSpeechRecognizerAuthorizationStatus))callback {
    [authorizations addObject:[callback copy]];
}
- (BOOL)fixture_supportsOnDeviceRecognition { return YES; }
- (SFSpeechRecognitionTask *)fixture_recognitionTaskWithRequest:(SFSpeechRecognitionRequest *)request
    resultHandler:(void (^)(SFSpeechRecognitionResult *, NSError *))callback {
    NSCAssert(request.requiresOnDeviceRecognition, @"Cloud recognition must remain disabled");
    [recognitions addObject:[callback copy]];
    return [FixtureTask new];
}
@end

void SpeechFixtureInstall(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        method_exchangeImplementations(class_getClassMethod(SFSpeechRecognizer.class, @selector(requestAuthorization:)),
            class_getClassMethod(SFSpeechRecognizer.class, @selector(fixture_authorize:)));
        method_exchangeImplementations(class_getInstanceMethod(SFSpeechRecognizer.class, @selector(supportsOnDeviceRecognition)),
            class_getInstanceMethod(SFSpeechRecognizer.class, @selector(fixture_supportsOnDeviceRecognition)));
        method_exchangeImplementations(class_getInstanceMethod(SFSpeechRecognizer.class, @selector(recognitionTaskWithRequest:resultHandler:)),
            class_getInstanceMethod(SFSpeechRecognizer.class, @selector(fixture_recognitionTaskWithRequest:resultHandler:)));
    });
    SpeechFixtureReset();
}
void SpeechFixtureReset(void) {
    authorizations = [NSMutableArray new]; recognitions = [NSMutableArray new]; cancellations = 0;
}
NSUInteger SpeechFixtureAuthorizationCount(void) { return authorizations.count; }
NSUInteger SpeechFixtureRecognitionCount(void) { return recognitions.count; }
NSUInteger SpeechFixtureCancellationCount(void) { return cancellations; }
void SpeechFixtureAuthorize(NSUInteger index, SFSpeechRecognizerAuthorizationStatus status, dispatch_block_t delivered) {
    void (^callback)(SFSpeechRecognizerAuthorizationStatus) = authorizations[index];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{ dispatch_assert_queue_not(dispatch_get_main_queue()); callback(status); delivered(); });
}
void SpeechFixtureRecognize(NSUInteger index, NSString *text, BOOL final, BOOL failed, dispatch_block_t delivered) {
    void (^callback)(SFSpeechRecognitionResult *, NSError *) = recognitions[index];
    FixtureTranscript *transcript = [FixtureTranscript new]; transcript.fixtureText = text;
    FixtureResult *result = [FixtureResult new]; result.fixtureTranscript = transcript; result.fixtureFinal = final;
    NSError *error = failed ? [NSError errorWithDomain:@"SpeechCallbackFixture" code:1 userInfo:nil] : nil;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{ dispatch_assert_queue_not(dispatch_get_main_queue()); callback(result, error); delivered(); });
}
