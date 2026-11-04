// System audio capture with Core Audio process taps (macOS 14.2+).
//
// A tap is a HAL audio object that mirrors the output of other processes. It is
// attached to a private aggregate device, which is read with a normal IOProc.
// No virtual driver (BlackHole, Soundflower) is involved.
//
// The C ABI is kept tiny on purpose: Rust owns the channel and the resampling,
// this file only converts the HAL buffer into a mono f32 callback.

#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <AudioToolbox/AudioToolbox.h>

#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>

typedef void (*scrib_audio_callback)(const float *samples,
                                    uint32_t frames,
                                    uint32_t channels,
                                    double sample_rate,
                                    void *context);

typedef struct {
    AudioObjectID tap_id;
    AudioObjectID aggregate_id;
    AudioDeviceIOProcID proc_id;
    AudioStreamBasicDescription format;
    scrib_audio_callback callback;
    void *context;
    int started;
} ScribSystemTap;

static void set_error(char *error, size_t error_len, NSString *message) {
    if (error == NULL || error_len == 0) {
        return;
    }
    const char *text = message.UTF8String;
    snprintf(error, error_len, "%s", text != NULL ? text : "unknown system audio error");
}

static AudioObjectPropertyAddress global_property(AudioObjectPropertySelector selector) {
    AudioObjectPropertyAddress address = {
        .mSelector = selector,
        .mScope = kAudioObjectPropertyScopeGlobal,
        .mElement = kAudioObjectPropertyElementMain,
    };
    return address;
}

static OSStatus read_tap_format(AudioObjectID tap_id, AudioStreamBasicDescription *format) {
    AudioObjectPropertyAddress address = global_property(kAudioTapPropertyFormat);
    UInt32 size = (UInt32)sizeof(*format);
    return AudioObjectGetPropertyData(tap_id, &address, 0, NULL, &size, format);
}

static OSStatus read_tap_uid(AudioObjectID tap_id, CFStringRef *uid) {
    AudioObjectPropertyAddress address = global_property(kAudioTapPropertyUID);
    UInt32 size = (UInt32)sizeof(*uid);
    return AudioObjectGetPropertyData(tap_id, &address, 0, NULL, &size, uid);
}

static OSStatus read_default_output_uid(CFStringRef *uid) {
    AudioObjectID device = kAudioObjectUnknown;
    AudioObjectPropertyAddress address =
        global_property(kAudioHardwarePropertyDefaultSystemOutputDevice);
    UInt32 size = (UInt32)sizeof(device);
    OSStatus status =
        AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device);
    if (status != noErr) {
        return status;
    }
    if (device == kAudioObjectUnknown) {
        return kAudioHardwareBadDeviceError;
    }
    address = global_property(kAudioDevicePropertyDeviceUID);
    size = (UInt32)sizeof(*uid);
    return AudioObjectGetPropertyData(device, &address, 0, NULL, &size, uid);
}

static OSStatus tap_io_proc(AudioObjectID device,
                            const AudioTimeStamp *now,
                            const AudioBufferList *input,
                            const AudioTimeStamp *input_time,
                            AudioBufferList *output,
                            const AudioTimeStamp *output_time,
                            void *client_data) {
    (void)device;
    (void)now;
    (void)input_time;
    (void)output;
    (void)output_time;

    ScribSystemTap *tap = (ScribSystemTap *)client_data;
    if (tap == NULL || input == NULL || input->mNumberBuffers == 0) {
        return noErr;
    }
    const AudioBuffer *buffer = &input->mBuffers[0];
    UInt32 channels = tap->format.mChannelsPerFrame;
    if (buffer->mData == NULL || buffer->mDataByteSize == 0 || channels == 0) {
        return noErr;
    }
    UInt32 frames = buffer->mDataByteSize / (UInt32)(sizeof(float) * channels);
    if (frames > 0) {
        tap->callback((const float *)buffer->mData,
                      frames,
                      channels,
                      tap->format.mSampleRate,
                      tap->context);
    }
    return noErr;
}

static void destroy_tap(ScribSystemTap *tap) {
    if (tap == NULL) {
        return;
    }
    if (tap->proc_id != NULL) {
        if (tap->started) {
            AudioDeviceStop(tap->aggregate_id, tap->proc_id);
        }
        AudioDeviceDestroyIOProcID(tap->aggregate_id, tap->proc_id);
    }
    if (tap->aggregate_id != kAudioObjectUnknown) {
        AudioHardwareDestroyAggregateDevice(tap->aggregate_id);
    }
    if (tap->tap_id != kAudioObjectUnknown) {
        AudioHardwareDestroyProcessTap(tap->tap_id);
    }
    free(tap);
}

int scrib_system_audio_available(void) {
    if (@available(macOS 14.2, *)) {
        return 1;
    }
    return 0;
}

// TCCAccessPreflight is a private TCC SPI: there is no public API to check the
// System Audio Recording grant. The result is 0 authorized, 1 denied, 2 not
// determined; -2 means the SPI is unavailable and must not block the caller.
typedef int (*scrib_tcc_preflight)(CFStringRef, CFDictionaryRef);

int scrib_system_audio_permission(void) {
    static scrib_tcc_preflight preflight = NULL;
    static int loaded = 0;
    if (!loaded) {
        loaded = 1;
        void *handle = dlopen("/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC",
                              RTLD_NOW);
        if (handle != NULL) {
            preflight = (scrib_tcc_preflight)dlsym(handle, "TCCAccessPreflight");
        }
    }
    if (preflight == NULL) {
        return -2;
    }
    return preflight(CFSTR("kTCCServiceAudioCapture"), NULL);
}

void *scrib_system_audio_create(scrib_audio_callback callback,
                                void *context,
                                char *error,
                                size_t error_len) {
    if (@available(macOS 14.2, *)) {
        // Fall through to the implementation below.
    } else {
        set_error(error, error_len, @"system audio capture requires macOS 14.2 or later");
        return NULL;
    }

    ScribSystemTap *tap = calloc(1, sizeof(ScribSystemTap));
    if (tap == NULL) {
        set_error(error, error_len, @"could not allocate the system audio tap");
        return NULL;
    }
    tap->tap_id = kAudioObjectUnknown;
    tap->aggregate_id = kAudioObjectUnknown;
    tap->callback = callback;
    tap->context = context;

    CATapDescription *description = [[CATapDescription alloc] init];
    description.name = @"Scribroom system audio";
    // An exclusive global tap with no excluded processes captures every process.
    description.processes = @[];
    description.privateTap = YES;
    description.exclusive = YES;
    description.mixdown = YES;
    description.mono = YES;
    description.muteBehavior = CATapUnmuted;

    OSStatus status = AudioHardwareCreateProcessTap(description, &tap->tap_id);
    if (status != noErr) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not create the system audio tap (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }

    status = read_tap_format(tap->tap_id, &tap->format);
    if (status != noErr) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not read the system audio tap format (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }
    // The IOProc below reads the first buffer as interleaved f32. A mono tap is
    // guaranteed to match that layout; anything else is rejected instead of
    // being decoded as noise.
    if (tap->format.mFormatID != kAudioFormatLinearPCM ||
        (tap->format.mFormatFlags & kAudioFormatFlagIsFloat) == 0 ||
        tap->format.mBitsPerChannel != 32 ||
        tap->format.mChannelsPerFrame != 1) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"unsupported system audio tap format (format %u, flags %u, %u bits, %u channels)",
                                             (unsigned)tap->format.mFormatID,
                                             (unsigned)tap->format.mFormatFlags,
                                             (unsigned)tap->format.mBitsPerChannel,
                                             (unsigned)tap->format.mChannelsPerFrame]);
        destroy_tap(tap);
        return NULL;
    }

    CFStringRef tap_uid = NULL;
    status = read_tap_uid(tap->tap_id, &tap_uid);
    if (status != noErr || tap_uid == NULL) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not read the system audio tap identifier (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }

    CFStringRef output_uid = NULL;
    status = read_default_output_uid(&output_uid);
    if (status != noErr || output_uid == NULL) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not read the default output device (OSStatus %d)", (int)status]);
        CFRelease(tap_uid);
        destroy_tap(tap);
        return NULL;
    }

    // Build the aggregate with the tap list already in place. Attaching the tap
    // afterwards is racy: Core Audio may not expose the input stream yet and
    // AudioDeviceStart fails with kAudioHardwareIllegalOperationError.
    // The main subdevice also gives the aggregate a clock.
    NSDictionary *tap_entry = @{
        [NSString stringWithUTF8String:kAudioSubTapUIDKey]: (__bridge NSString *)tap_uid,
        [NSString stringWithUTF8String:kAudioSubTapDriftCompensationKey]: @YES,
    };
    NSDictionary *subdevice = @{
        [NSString stringWithUTF8String:kAudioSubDeviceUIDKey]: (__bridge NSString *)output_uid,
    };
    NSString *aggregate_uid = [NSString stringWithFormat:@"scrib-system-audio-%@", NSUUID.UUID.UUIDString];
    NSDictionary *composition = @{
        [NSString stringWithUTF8String:kAudioAggregateDeviceNameKey]: @"Scribroom System Audio",
        [NSString stringWithUTF8String:kAudioAggregateDeviceUIDKey]: aggregate_uid,
        [NSString stringWithUTF8String:kAudioAggregateDeviceMainSubDeviceKey]: (__bridge NSString *)output_uid,
        [NSString stringWithUTF8String:kAudioAggregateDeviceSubDeviceListKey]: @[subdevice],
        [NSString stringWithUTF8String:kAudioAggregateDeviceIsPrivateKey]: @YES,
        [NSString stringWithUTF8String:kAudioAggregateDeviceIsStackedKey]: @NO,
        [NSString stringWithUTF8String:kAudioAggregateDeviceTapListKey]: @[tap_entry],
    };
    status = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)composition, &tap->aggregate_id);
    CFRelease(tap_uid);
    CFRelease(output_uid);
    if (status != noErr) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not create the system audio aggregate device (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }

    UInt32 stream_list_size = 0;
    AudioObjectPropertyAddress input_streams = global_property(kAudioDevicePropertyStreams);
    input_streams.mScope = kAudioObjectPropertyScopeInput;
    status = AudioObjectGetPropertyDataSize(tap->aggregate_id,
                                            &input_streams,
                                            0,
                                            NULL,
                                            &stream_list_size);
    if (status != noErr || stream_list_size == 0) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"the aggregate device has no input stream for the tap (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }

    status = AudioDeviceCreateIOProcID(tap->aggregate_id, tap_io_proc, tap, &tap->proc_id);
    if (status != noErr) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not create the system audio IOProc (OSStatus %d)", (int)status]);
        destroy_tap(tap);
        return NULL;
    }

    return tap;
}

int scrib_system_audio_begin(void *handle, char *error, size_t error_len) {
    ScribSystemTap *tap = (ScribSystemTap *)handle;
    if (tap == NULL) {
        set_error(error, error_len, @"the system audio tap is not initialized");
        return 1;
    }
    if (tap->started) {
        return 0;
    }
    OSStatus status = AudioDeviceStart(tap->aggregate_id, tap->proc_id);
    if (status != noErr) {
        set_error(error, error_len,
                  [NSString stringWithFormat:@"could not start the system audio device (OSStatus %d)", (int)status]);
        return 1;
    }
    tap->started = 1;
    return 0;
}

void scrib_system_audio_stop(void *handle) {
    destroy_tap((ScribSystemTap *)handle);
}
