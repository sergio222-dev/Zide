//! The sound a chat plays when its agent finishes working.

use std::io::Cursor;

static DONE: &[u8] = include_bytes!("../../../assets/done.mp3");

/// Plays the "done" sound once. It runs on its own thread, so neither the audio device opening
/// nor the playback holds the UI; a machine without audio output stays silent.
pub fn done() {
    std::thread::spawn(|| {
        let Ok(mut sink) = rodio::DeviceSinkBuilder::open_default_sink() else {
            return;
        };
        sink.log_on_drop(false);
        let Ok(decoder) = rodio::Decoder::new(Cursor::new(DONE)) else {
            return;
        };
        let player = rodio::Player::connect_new(sink.mixer());
        player.append(decoder);
        player.sleep_until_end();
    });
}
