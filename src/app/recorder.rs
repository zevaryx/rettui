//! Recording voice messages in the terminal UI (`v` in a conversation).
//!
//! rettui doesn't link a sound library (a server or a Docker image has
//! none to link), so it runs a recorder program for its output: raw 16-bit
//! little-endian mono 8 kHz sound, for as long as it's wanted. That's the
//! `voice_recorder` setting's command, or else the first of these that's
//! installed: PulseAudio's (and PipeWire's) `parecord`, ALSA's `arecord`,
//! SoX's `rec` and `ffmpeg`. The sound goes as Codec2, as one recorded in
//! the web UI does.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::lxmf::voice::MAX_SEND_SECONDS;

use super::App;

/// Bytes of sound a second: 8000 samples of 2 bytes.
const BYTES_A_SECOND: usize = 16_000;
/// Most sound kept (and then the recording stops).
const MAX_BYTES: usize = MAX_SEND_SECONDS * BYTES_A_SECOND;
/// The end of what a recorder says on its error output that's kept.
const ERROR_CHARS: usize = 400;

/// The recorders tried, in order, when the setting is empty.
fn recorders() -> Vec<Vec<String>> {
    fn sox<'a>(input: &[&'a str]) -> Vec<&'a str> {
        let mut args: Vec<&'a str> = input.to_vec();
        args.extend(["-q", "-t", "raw", "-e", "signed-integer", "-b", "16", "-L", "-c", "1", "-r", "8000", "-"]);
        args
    }
    fn ffmpeg<'a>(format: &'a str, device: &'a str) -> Vec<&'a str> {
        vec!["ffmpeg", "-nostdin", "-loglevel", "error", "-f", format, "-i", device, "-ac", "1", "-ar", "8000", "-f", "s16le", "-"]
    }
    let list: Vec<Vec<&str>> = if cfg!(windows) {
        vec![sox(&["sox", "-q", "-t", "waveaudio", "default"])]
    } else if cfg!(target_os = "macos") {
        vec![sox(&["rec"]), ffmpeg("avfoundation", ":0")]
    } else {
        vec![
            vec!["parecord", "--raw", "--format=s16le", "--rate=8000", "--channels=1"],
            vec!["arecord", "-q", "-t", "raw", "-f", "S16_LE", "-c", "1", "-r", "8000"],
            sox(&["rec"]),
            ffmpeg("pulse", "default"),
        ]
    };
    list.into_iter().map(|args| args.into_iter().map(String::from).collect()).collect()
}

/// A command's words: split at spaces, except in quotes.
fn words(command: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => (quote, any) = (Some(c), true),
            (None, c) if c.is_whitespace() => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            (None, c) => word.push(c),
        }
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    words
}

/// How a recording is going.
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    Recording,
    /// It stopped by itself with sound (the recorder ended, or the longest
    /// recording was reached).
    Done,
    Failed(String),
}

/// A recorder running.
pub struct Recording {
    child: Child,
    sound: Arc<Mutex<Vec<u8>>>,
    errors: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    /// The program recording (its name).
    pub program: String,
    /// Recorders still to try if this one gives up at once.
    rest: Vec<Vec<String>>,
}

impl Recording {
    /// Start recording with `command` (the setting), or else the first
    /// recorder found.
    pub fn start(command: Option<&str>) -> Result<Self, String> {
        let custom = command.map(words).filter(|w| !w.is_empty());
        let mut list = match &custom {
            Some(words) => vec![words.clone()],
            None => recorders(),
        };
        list.reverse();
        Self::next(list, custom.is_some())
    }

    /// Start the last of `rest` that can be run.
    fn next(mut rest: Vec<Vec<String>>, custom: bool) -> Result<Self, String> {
        while let Some(args) = rest.pop() {
            match spawn(&args, rest.clone()) {
                Ok(recording) => return Ok(recording),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !custom => continue,
                Err(e) => return Err(format!("Couldn't run {}: {e}", args[0])),
            }
        }
        Err("No recorder found: install parecord (PulseAudio or PipeWire), arecord (ALSA), SoX or ffmpeg, or set Voice recorder in Settings".into())
    }

    /// Seconds of sound so far.
    pub fn seconds(&self) -> usize {
        self.bytes() / BYTES_A_SECOND
    }

    /// How it's going; a recorder that gave up at once, with nothing, makes
    /// way for the next one found.
    pub fn poll(&mut self) -> Progress {
        let status = match self.child.try_wait() {
            Ok(None) if self.bytes() >= MAX_BYTES => return Progress::Done,
            Ok(None) => return Progress::Recording,
            Ok(Some(status)) => status,
            Err(e) => return Progress::Failed(format!("{}: {e}", self.program)),
        };
        self.finish_reading();
        if self.bytes() > 0 {
            return Progress::Done;
        }
        let said = self.said();
        if !self.rest.is_empty()
            && let Ok(next) = Self::next(std::mem::take(&mut self.rest), false)
        {
            *self = next;
            return Progress::Recording;
        }
        Progress::Failed(match said {
            Some(said) => format!("{} stopped: {said}", self.program),
            None => format!("{} stopped ({status}) without recording anything", self.program),
        })
    }

    fn bytes(&self) -> usize {
        self.sound.lock().map_or(0, |s| s.len())
    }

    /// Wait (a moment at most: a program the recorder started could keep
    /// its output open) for the last of its output to be read.
    fn finish_reading(&mut self) {
        for _ in 0..50 {
            if self.readers.iter().all(JoinHandle::is_finished) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// What the recorder said on its error output, if anything.
    fn said(&self) -> Option<String> {
        let said = self.errors.lock().map(|e| e.trim().to_string()).unwrap_or_default();
        (!said.is_empty()).then(|| said.lines().last().unwrap_or_default().to_string())
    }

    /// Stop: the sound (8 kHz samples), if any was recorded.
    pub fn stop(mut self) -> Result<Vec<i16>, String> {
        self.end();
        let sound = std::mem::take(&mut *self.sound.lock().map_err(|_| "The recording was lost")?);
        if sound.len() < 2 {
            return Err(match self.said() {
                Some(said) => format!("Nothing was recorded: {} said {said}", self.program),
                None => "Nothing was recorded".into(),
            });
        }
        Ok(sound.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect())
    }

    /// Stop the recorder and wait for the last of its sound.
    fn end(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.finish_reading();
    }
}

impl Drop for Recording {
    // A recorder isn't left running (when rettui quits while recording).
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run a recorder, keeping its sound (up to [`MAX_BYTES`]) and the end of
/// what it says on its error output.
fn spawn(args: &[String], rest: Vec<Vec<String>>) -> std::io::Result<Recording> {
    let mut child = Command::new(&args[0]).args(&args[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (sound, errors) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(String::new())));
    let mut out = child.stdout.take().expect("piped");
    let kept = sound.clone();
    let sound_reader = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n @ 1..) = out.read(&mut buffer) {
            let Ok(mut sound) = kept.lock() else { return };
            let room = MAX_BYTES - sound.len();
            sound.extend_from_slice(&buffer[..n.min(room)]);
            if sound.len() >= MAX_BYTES {
                return;
            }
        }
    });
    let mut err = child.stderr.take().expect("piped");
    let said = errors.clone();
    let error_reader = std::thread::spawn(move || {
        let mut buffer = [0u8; 1024];
        while let Ok(n @ 1..) = err.read(&mut buffer) {
            let Ok(mut said) = said.lock() else { return };
            said.push_str(&String::from_utf8_lossy(&buffer[..n]));
            let extra = said.chars().count().saturating_sub(ERROR_CHARS);
            if extra > 0 {
                *said = said.chars().skip(extra).collect();
            }
        }
    });
    let program = std::path::Path::new(&args[0]).file_name().map_or(args[0].clone(), |n| n.to_string_lossy().into_owned());
    Ok(Recording { child, sound, errors, readers: vec![sound_reader, error_reader], program, rest })
}

/// A recording made, to send to `key` with what's written.
pub struct VoiceDraft {
    pub key: String,
    pub samples: Vec<i16>,
}

impl VoiceDraft {
    pub fn seconds(&self) -> usize {
        self.samples.len().div_ceil(8000)
    }
}

impl App {
    /// `v`: start recording a voice message to the open conversation, or
    /// stop and keep it to send.
    pub(super) fn toggle_recording(&mut self) {
        if self.recording.is_some() {
            return self.stop_recording(true);
        }
        let Some(key) = self.active_conversation.clone() else { return };
        if !self.attachments.is_empty() {
            return self.warn("A voice message goes on its own, or with text: take the files off first (Ctrl+X)");
        }
        match Recording::start(self.settings.voice_recorder.as_deref()) {
            Ok(recording) => {
                self.confirm(format!("Recording with {}: v stops, Esc throws it away", recording.program));
                self.recording = Some((key, recording));
                self.voice_draft = None;
            }
            Err(e) => self.warn(e),
        }
    }

    /// Stop recording, and keep what was recorded to send (then write
    /// anything to go with it, and Enter sends), or throw it away.
    pub(super) fn stop_recording(&mut self, keep: bool) {
        let Some((key, recording)) = self.recording.take() else { return };
        if !keep {
            drop(recording);
            return self.confirm("Recording thrown away");
        }
        match recording.stop() {
            Ok(samples) => {
                let draft = VoiceDraft { key, samples };
                self.confirm(format!("{} s recorded: Enter sends it, with anything written; Ctrl+X throws it away", draft.seconds()));
                self.voice_draft = Some(draft);
                self.composing = true;
            }
            Err(e) => self.warn(e),
        }
    }

    /// Keep an eye on the recorder: it may have stopped by itself.
    pub(super) fn recording_tick(&mut self) {
        let Some((_, recording)) = self.recording.as_mut() else { return };
        match recording.poll() {
            Progress::Recording => {}
            Progress::Done => self.stop_recording(true),
            Progress::Failed(e) => {
                self.recording = None;
                self.warn(e);
            }
        }
    }

    /// The voice message recorded for the open conversation, if any.
    pub fn voice_draft_here(&self) -> Option<&VoiceDraft> {
        self.voice_draft.as_ref().filter(|d| self.active_conversation.as_deref() == Some(d.key.as_str()))
    }

    /// Seconds recorded so far, if recording.
    pub fn recording_seconds(&self) -> Option<usize> {
        self.recording.as_ref().map(|(_, r)| r.seconds())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_split_into_words() {
        assert_eq!(words("arecord -q -r 8000"), ["arecord", "-q", "-r", "8000"]);
        assert_eq!(words("  sox  -t \"wave audio\" ''  'x y'"), ["sox", "-t", "wave audio", "", "x y"]);
        assert!(words("   ").is_empty());
        assert!(recorders().iter().all(|r| !r.is_empty()));
    }

    #[cfg(unix)]
    fn wait(recording: &mut Recording) -> Progress {
        for _ in 0..200 {
            match recording.poll() {
                Progress::Recording => std::thread::sleep(std::time::Duration::from_millis(10)),
                done => return done,
            }
        }
        Progress::Recording
    }

    #[cfg(unix)]
    #[test]
    fn a_recorders_sound_is_kept() {
        // Half a second of sound (silence), then it ends by itself.
        let mut recording = Recording::start(Some("head -c 8000 /dev/zero")).unwrap();
        assert_eq!(recording.program, "head");
        assert_eq!(wait(&mut recording), Progress::Done);
        assert_eq!(recording.stop().unwrap().len(), 4000);
        // Stopped while it goes on: what it gave so far.
        let recording = Recording::start(Some("cat /dev/zero")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!recording.stop().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_recorder_that_fails_says_why() {
        let missing = Recording::start(Some("rettui-no-such-recorder")).err().unwrap();
        assert!(missing.starts_with("Couldn't run rettui-no-such-recorder"), "{missing}");
        let mut recording = Recording::start(Some("sh -c 'echo no microphone >&2; exit 1'")).unwrap();
        assert_eq!(wait(&mut recording), Progress::Failed("sh stopped: no microphone".into()));
        let recording = Recording::start(Some("sh -c 'echo no microphone >&2; exec sleep 5'")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(recording.stop().err().unwrap(), "Nothing was recorded: sh said no microphone");
        // The next recorder found is tried when one gives up at once.
        let mut recording =
            Recording::next(vec![vec!["head".into(), "-c".into(), "100".into(), "/dev/zero".into()], vec!["false".into()]], false).unwrap();
        assert_eq!(recording.program, "false");
        assert_eq!(wait(&mut recording), Progress::Done);
        assert_eq!(recording.program, "head");
    }
}
