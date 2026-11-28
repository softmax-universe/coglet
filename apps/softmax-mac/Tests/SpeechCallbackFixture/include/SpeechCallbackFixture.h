#import <Foundation/Foundation.h>
#import <Speech/Speech.h>

NS_ASSUME_NONNULL_BEGIN
// Test-process-only replacement of the Speech SDK. No TCC or hardware calls.
void SpeechFixtureInstall(void);
void SpeechFixtureReset(void);
NSUInteger SpeechFixtureAuthorizationCount(void);
NSUInteger SpeechFixtureRecognitionCount(void);
NSUInteger SpeechFixtureCancellationCount(void);
void SpeechFixtureAuthorize(NSUInteger index, SFSpeechRecognizerAuthorizationStatus status, dispatch_block_t delivered);
void SpeechFixtureRecognize(NSUInteger index, NSString *text, BOOL final, BOOL failed, dispatch_block_t delivered);
NS_ASSUME_NONNULL_END
