use byteorder::{BigEndian, WriteBytesExt};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Display;
use std::io::Write;

use flvmux::{AacAudioPacketType, AvcPacketType};

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

pub struct FifoMixer {
    source_timestamps: HashMap<MixerSource, SourceTs>,
    audio_timestamp: i32,
    video_timestamp: i32,
    current_video_source: Option<MixerSource>,
    current_audio_source: Option<MixerSource>,
}

impl Default for FifoMixer {
    fn default() -> Self {
        Self {
            source_timestamps: HashMap::new(),
            audio_timestamp: 0,
            video_timestamp: 0,
            current_video_source: None,
            current_audio_source: None,
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

        match flvmux::read_audio_header(data)? {
            AacAudioPacketType::SequenceHeader if self.current_audio_source.is_none() => {
                // Ok
            }
            AacAudioPacketType::Raw => {
                // Ok
            }
            _ => return Ok(()),
        }

        self.current_audio_source = Some(source);
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
            AvcPacketType::SequenceHeader if self.current_video_source.is_none() => {}
            AvcPacketType::Nalu { seekable: true, .. } => {}
            AvcPacketType::Nalu { .. } if Some(source) == self.current_video_source => {
                // Ok
            }
            _ => return Ok(()),
        }

        self.current_video_source = Some(source);
        self.video_timestamp += dt;
        let data_size = u32::try_from(data.len())?;
        flvmux::write_video_tag_header(&mut out, data_size, self.video_timestamp)?;
        out.write_all(data)?;
        let data_size = u32::try_from(data.len())?;
        out.write_u32::<BigEndian>(data_size + 11)?; // 11 bytes of header

        Ok(())
    }
}
