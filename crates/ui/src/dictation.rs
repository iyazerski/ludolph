//! Dictation with Apple's speech recognizer: the microphone feeds it, and it
//! reports what it hears so far. Winit's text input drops most of what macOS
//! system dictation types, so the app listens itself.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::NSError;
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use unicode_segmentation::UnicodeSegmentation;

/// The recognizer's error for a session that ended before any speech.
const NO_SPEECH: isize = 1110;
/// Where System Settings lets the user change their answer.
const SETTINGS: &str = "Allow Ludolph in System Settings ▸ Privacy & Security";

pub enum Heard {
    /// Everything heard since the session started, as text.
    Words(String),
    /// How loud the microphone is, from 0 to 1, many times a second.
    Level(f32),
    /// The session is over and its words are final.
    Done,
    Failed(String),
}

/// A running session; dropping it stops listening and drops what's unheard.
pub struct Listening {
    engine: Retained<AVAudioEngine>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
}

/// Ask for speech recognition and the microphone, then call `done` on any
/// thread with why dictation can't run, if it can't.
pub fn authorize(done: impl FnOnce(Allowed) + Send + 'static) {
    let done = Once::new(done);
    // SAFETY: the status is a plain class property.
    match unsafe { SFSpeechRecognizer::authorizationStatus() } {
        SFSpeechRecognizerAuthorizationStatus::Authorized => microphone(done),
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => {
            let asked = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
                if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
                    microphone(done.clone());
                } else {
                    done.call(Err(format!("Speech recognition is off. {SETTINGS}.")));
                }
            });
            // SAFETY: the binary's Info.plist explains the request, which
            // macOS requires; the block is copied and may run on any queue.
            unsafe { SFSpeechRecognizer::requestAuthorization(&asked) };
        }
        _ => done.call(Err(format!("Speech recognition is off. {SETTINGS}."))),
    }
}

fn microphone(done: Once) {
    // SAFETY: AVFoundation defines the audio media type for the app's life.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return done.call(Err("No audio input on this Mac".into()));
    };
    let denied = || Err(format!("The microphone is off. {SETTINGS}."));
    // SAFETY: a class property, read with a valid media type.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => done.call(Ok(())),
        AVAuthorizationStatus::NotDetermined => {
            let asked = RcBlock::new(move |granted: Bool| {
                done.call(if granted.as_bool() { Ok(()) } else { denied() });
            });
            // SAFETY: as for speech recognition: the Info.plist explains it,
            // and the block may run on any queue.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &asked) };
        }
        _ => done.call(denied()),
    }
}

impl Listening {
    /// Listen to the microphone, calling `heard` with what it hears until the
    /// session ends: on the audio thread for levels, else on the main thread.
    /// Needs `authorize` to have passed.
    pub fn start(heard: impl Fn(Heard) + Send + Sync + 'static) -> Result<Self, String> {
        let heard = Arc::new(heard);
        // SAFETY: plain Speech and AVFAudio calls on the main thread, with
        // valid objects. The recognizer reports on the main queue, its default.
        unsafe {
            let recognizer = SFSpeechRecognizer::new();
            if !recognizer.isAvailable() {
                return Err("Speech recognition isn't available right now".into());
            }
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setShouldReportPartialResults(true);
            request.setAddsPunctuation(true);
            // On the Mac, when it can: private, and with no time limit.
            if recognizer.supportsOnDeviceRecognition() {
                request.setRequiresOnDeviceRecognition(true);
            }
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let format = input.outputFormatForBus(0);
            let feed = request.clone();
            let loud = heard.clone();
            let tap = RcBlock::new(
                move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
                    let buffer = buffer.as_ref();
                    feed.appendAudioPCMBuffer(buffer);
                    // Each buffer, a small fraction of a second, draws one bar.
                    if let Some(level) = level(buffer) {
                        loud(Heard::Level(level));
                    }
                },
            );
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&tap),
            );
            let transcript = RefCell::new(Transcript::default());
            let handler = RcBlock::new(
                move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                    if let Some(result) = result.as_ref() {
                        let current = result.bestTranscription().formattedString().to_string();
                        // Metadata comes with the last result before a pause.
                        let pause = result.speechRecognitionMetadata().is_some();
                        let words = transcript.borrow_mut().take(&current, pause);
                        heard(Heard::Words(words));
                        if result.isFinal() {
                            heard(Heard::Done);
                        }
                    } else if let Some(error) = error.as_ref() {
                        heard(if error.code() == NO_SPEECH {
                            Heard::Done
                        } else {
                            Heard::Failed(error.localizedDescription().to_string())
                        });
                    }
                },
            );
            let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler);
            engine.prepare();
            if let Err(e) = engine.startAndReturnError() {
                input.removeTapOnBus(0);
                task.cancel();
                return Err(e.localizedDescription().to_string());
            }
            Ok(Self {
                engine,
                request,
                task,
            })
        }
    }

    /// Stop listening; the last words and `Heard::Done` follow.
    pub fn stop(&self) {
        // SAFETY: valid objects; stopping twice is harmless.
        unsafe {
            self.engine.stop();
            self.engine.inputNode().removeTapOnBus(0);
            self.request.endAudio();
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop();
        // SAFETY: a valid task; cancelling a finished one does nothing.
        unsafe { self.task.cancel() };
    }
}

/// How loud `buffer` is, from 0 for silence to 1 for loud speech.
///
/// # Safety
/// `buffer` must be a valid buffer from the tap, alive for the call.
unsafe fn level(buffer: &AVAudioPCMBuffer) -> Option<f32> {
    // SAFETY: the tap's buffers hold float samples, `frameLength` of them in
    // each channel; the first channel is enough.
    let samples = unsafe {
        let channels = NonNull::new(buffer.floatChannelData())?;
        std::slice::from_raw_parts(channels.read().as_ptr(), buffer.frameLength() as usize)
    };
    if samples.is_empty() {
        return None;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    // Speech sits between about -50 dBFS (quiet) and -10 dBFS (loud).
    let db = 20.0 * rms.max(1e-6).log10();
    Some(((db + 50.0) / 40.0).clamp(0.0, 1.0))
}

/// What a session heard so far. The recognizer starts a new transcription
/// after each pause, and once listening stops, repeats the last one or all
/// of them as final.
#[derive(Default)]
struct Transcript {
    /// Everything before the current transcription.
    said: String,
    /// The words of `said`, kept so each result doesn't split it again.
    said_words: Vec<String>,
    /// The words of the transcription kept at the last pause, until another
    /// starts.
    kept: Option<Vec<String>>,
}

impl Transcript {
    /// Take a result's transcription, `pause` when it's the last before a
    /// pause, and return everything heard.
    fn take(&mut self, current: &str, pause: bool) -> String {
        let current_words = words(current);
        if self.kept.as_ref() == Some(&current_words) {
            return self.said.clone();
        }
        if !self.said.is_empty() && current_words.starts_with(&self.said_words) {
            return current.to_owned();
        }
        let heard = join(&self.said, current);
        if pause {
            self.said = heard.clone();
            self.said_words.extend_from_slice(&current_words);
            self.kept = Some(current_words);
        } else {
            self.kept = None;
        }
        heard
    }
}

/// The words of `s`, to compare transcriptions whatever their case and punctuation.
fn words(s: &str) -> Vec<String> {
    s.unicode_words().map(str::to_lowercase).collect()
}

/// `before`, then `after`, with a space between when both have text.
pub fn join(before: &str, after: &str) -> String {
    match (before.trim_end(), after.trim_start()) {
        ("", after) => after.to_owned(),
        (before, "") => before.to_owned(),
        (before, after) => format!("{before} {after}"),
    }
}

type Allowed = Result<(), String>;
type Callback = Box<dyn FnOnce(Allowed) + Send>;

/// `authorize`'s callback, shared by blocks that AppKit may copy and call
/// more than once; it runs once.
#[derive(Clone)]
struct Once(Arc<Mutex<Option<Callback>>>);

impl Once {
    fn new(f: impl FnOnce(Allowed) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(f)))))
    }

    fn call(&self, allowed: Allowed) {
        let f = self
            .0
            .lock()
            .expect("the dictation callback lock is poisoned")
            .take();
        if let Some(f) = f {
            f(allowed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Transcript;

    #[test]
    fn keeps_each_pause_once() {
        let mut t = Transcript::default();
        // (transcription, before a pause, everything heard)
        let steps = [
            ("Nice", false, "Nice"),
            ("Nice it works", true, "Nice it works"),
            ("Really", false, "Nice it works Really"),
            ("Really well", true, "Nice it works Really well"),
            // Stopping repeats the last transcription as final.
            ("Really well.", false, "Nice it works Really well"),
        ];
        for (current, pause, heard) in steps {
            assert_eq!(t.take(current, pause), heard, "after {current:?}");
        }
        let mut t = Transcript::default();
        t.take("Nice it works", true);
        // Or it repeats everything.
        assert_eq!(
            t.take("Nice, it works really well.", false),
            "Nice, it works really well."
        );
    }
}
