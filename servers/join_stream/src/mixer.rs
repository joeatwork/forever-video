use byteorder::{BigEndian, WriteBytesExt};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Display;
use std::io::Write;

use flvmux::{AacAudioPacketType, AvcPacketType};

const MIN_AUDIO_INTERVAL: i32 = 2000;

pub type MixerSource = usize;

#[derive(Debug)]
pub struct MixerError {
    message: String,
}

impl<T: Display> From<T> for MixerError {
    fn from(other: T) -> Self {
        Self {
            message: format!("{}", other),
        }
    }
}

// Uniqueness of MixerSources is up to the client.
pub trait Mixer {
    fn source_video(
        &mut self,
        out: impl Write,
        source: MixerSource,
        data: &[u8],
        timestamp: i32,
    ) -> Result<(), MixerError>;
    fn source_audio(
        &mut self,
        out: impl Write,
        source: MixerSource,
        data: &[u8],
        timestamp: i32,
    ) -> Result<(), MixerError>;
}

#[derive(Debug)]
struct SourceTs {
    audio_ts: i32,
    video_ts: i32,
}

#[derive(Debug, PartialEq, Eq)]
struct LastSwitch {
    current: MixerSource,
    started: i32,
}

trait LastSwitchInfo {
    fn ready_for_change(&self, source: MixerSource, now: i32) -> bool;
    fn same_source(&self, source: MixerSource) -> bool;
}

impl LastSwitchInfo for Option<LastSwitch> {
    fn ready_for_change(&self, source: MixerSource, now: i32) -> bool {
        match self {
            None => true,
            Some(switch) => switch.current != source && MIN_AUDIO_INTERVAL < now - switch.started,
        }
    }

    fn same_source(&self, source: MixerSource) -> bool {
        if let Some(switch) = self {
            switch.current == source
        } else {
            false
        }
    }
}

pub struct FifoMixer {
    source_timestamps: HashMap<MixerSource, SourceTs>,
    audio_timestamp: i32,
    video_timestamp: i32,
    last_video_switch: Option<LastSwitch>,
    last_audio_switch: Option<LastSwitch>,
}

impl Default for FifoMixer {
    fn default() -> Self {
        Self {
            source_timestamps: HashMap::new(),
            audio_timestamp: 0,
            video_timestamp: 0,
            last_video_switch: None,
            last_audio_switch: None,
        }
    }
}

// This assumes that the relevant resolution and color space and sample rate
// (and any other out-of-band stuff that decoders expect not to change
// during a stream) are the same for all sources.
impl Mixer for FifoMixer {
    fn source_audio(
        &mut self,
        mut out: impl Write,
        source: MixerSource,
        data: &[u8],
        timestamp: i32,
    ) -> Result<(), MixerError> {
        let ts = match self.source_timestamps.get_mut(&source) {
            Some(ts) => ts,
            None => {
                self.source_timestamps.insert(
                    source,
                    SourceTs {
                        audio_ts: 0,
                        video_ts: 0,
                    },
                );
                self.source_timestamps.get_mut(&source).unwrap()
            }
        };
        let dt = timestamp - ts.audio_ts;
        ts.audio_ts = timestamp;

        // TODO our switching scheme stalls if the current audio stream stops
        // (which we should expect) because this.audio_timestamp stops advancing.
        // The right thing to do is to check the duration of the audio being played,
        // detect when we've run out of audio, and then use the video timestamp to
        // jumpstart things.
        match flvmux::read_audio_header(data)? {
            AacAudioPacketType::SequenceHeader if self.last_audio_switch.is_none() => {
                self.last_audio_switch = Some(LastSwitch {
                    current: source,
                    started: self.audio_timestamp,
                });
            }
            AacAudioPacketType::Raw
                if self
                    .last_audio_switch
                    .ready_for_change(source, self.audio_timestamp) =>
            {
                eprintln!(
                    "Audio change {:?} {} {}",
                    self.last_audio_switch, source, self.audio_timestamp
                );
                self.last_audio_switch = Some(LastSwitch {
                    current: source,
                    started: self.audio_timestamp,
                })
            }
            AacAudioPacketType::Raw if self.last_audio_switch.same_source(source) => {
                // Ok, pass through
            }
            _ => return Ok(()),
        }

        self.audio_timestamp += dt;
        let data_size = u32::try_from(data.len())?;
        flvmux::write_audio_tag_header(&mut out, data_size, self.audio_timestamp)?;
        out.write_all(data)?;
        let data_size = u32::try_from(data.len())?;
        out.write_u32::<BigEndian>(data_size + 11)?; // 11 bytes of header

        Ok(())
    }

    fn source_video(
        &mut self,
        mut out: impl Write,
        source: MixerSource,
        data: &[u8],
        timestamp: i32,
    ) -> Result<(), MixerError> {
        let ts = match self.source_timestamps.get_mut(&source) {
            Some(ts) => ts,
            None => {
                self.source_timestamps.insert(
                    source,
                    SourceTs {
                        audio_ts: 0,
                        video_ts: 0,
                    },
                );
                self.source_timestamps.get_mut(&source).unwrap()
            }
        };
        let dt = timestamp - ts.video_ts;
        ts.video_ts = timestamp;

        match flvmux::read_video_header(data)? {
            AvcPacketType::SequenceHeader if self.last_video_switch.is_none() => {
                self.last_video_switch = Some(LastSwitch {
                    current: source,
                    started: self.video_timestamp,
                })
            }
            AvcPacketType::Nalu { seekable: true, .. } => {
                self.last_video_switch = Some(LastSwitch {
                    current: source,
                    started: self.video_timestamp,
                })
            }
            AvcPacketType::Nalu { .. } if self.last_video_switch.same_source(source) => {
                // Ok, pass though
            }
            _ => return Ok(()),
        }

        self.video_timestamp += dt;
        let data_size = u32::try_from(data.len())?;
        flvmux::write_video_tag_header(&mut out, data_size, self.video_timestamp)?;
        out.write_all(data)?;
        let data_size = u32::try_from(data.len())?;
        out.write_u32::<BigEndian>(data_size + 11)?; // 11 bytes of header

        Ok(())
    }
}
