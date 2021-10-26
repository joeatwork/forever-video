use byteorder::{BigEndian, ByteOrder, ReadBytesExt, WriteBytesExt};
use rand::seq::SliceRandom;
use rand::Rng;
use std::convert::TryFrom;
use std::env;
use std::fs::File;
use std::io;
use std::io::Cursor;
use std::io::{Read, Seek, SeekFrom, Write};

// THE PLAN - read an FLV, push out another FLV

// PHASE 0:
// [] Shuffle audio tags.

// PHASE 0.5:
// [] Shuffle the video tags and see if it works, or just makes the stream crummy.
//    (It's POSSIBLE that this will "just work")

// PHASE 1:
// [] update h264 decode time / presentation times (which means being smart about the content)
//    and then shuffling the video tags.

#[derive(Clone, Copy, Debug)]
struct FileRange {
    offset: u64,
    length: u32,
}

#[derive(Debug)]
struct VideoNaluTag {
    decode_timestamp: i32,
    composition_time_offset: i32,
    seekable: bool,
    range: FileRange,
}

#[derive(Clone, Copy, Debug)]
struct AudioTag {
    timestamp: i32,
    range: FileRange,
}

#[derive(Debug)]
struct SeekMap {
    audio_sequence_header: FileRange,
    video_sequence_header: FileRange,
    video_end_of_sequence: FileRange,
    end_of_sequence_timestamp: i32,
    audio_tags: Vec<AudioTag>,
    video_tags: Vec<VideoNaluTag>,
}

struct TagHeader {
    datasize: u32,
    tagtype: u8,
    decode_ts: i32,
}

enum AacAudioInfo {
    SequenceHeader,
    Raw,
}

enum AvcVideoInfo {
    SequenceHeader,
    Nalu {
        seekable: bool,
        composition_time_offset: i32,
    },
    EndOfSequence,
}

impl FileRange {
    /// Pulls the range from source and writes it to dest.
    /// Does *not* seek dest. Leaves source seeked to a random place.
    /// Return the number of bytes written on success.
    fn read(&self, mut source: impl Read + Seek, buf: &mut Vec<u8>) -> io::Result<()> {
        // Annoyed to be uselessly zeroing out this memory...
        buf.resize(usize::try_from(self.length).unwrap(), 0);
        source.seek(SeekFrom::Start(self.offset))?;
        source.read_exact(buf)?;

        Ok(())
    }
}

fn write_tag_with_timestamp(
    range: FileRange,
    timestamp: i32,
    mut source: impl Read + Seek,
    mut dest: impl Write,
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    range.read(&mut source, buf)?;

    BigEndian::write_u24(&mut buf[4..], (timestamp & 0xffffff) as u32);
    buf[7] = (timestamp >> 24 & 0xff) as u8;

    dest.write_all(buf)?;
    dest.write_u32::<BigEndian>(u32::try_from(buf.len()).unwrap())?;

    Ok(())
}

impl SeekMap {
    /// Dumps all known tags from inf to outf. Regular tags are dumped in timestamp order.
    fn dump(&self, mut source: impl Read + Seek, mut dest: impl Write) -> io::Result<()> {
        let mut buf = Vec::with_capacity(4096);

        // FLV file header
        dest.write_all(&[
            0x46, 0x4c, 0x56, // 'FLV'
            0x01, // version 1
            0x05, // use video and audio
            0x0, 0x0, 0x0,  // reserved
            0x09, // size of this header
        ])?;

        dest.write_u32::<BigEndian>(0)?; // First previous tag size

        write_tag_with_timestamp(
            self.video_sequence_header,
            0,
            &mut source,
            &mut dest,
            &mut buf,
        )?;
        write_tag_with_timestamp(
            self.audio_sequence_header,
            0,
            &mut source,
            &mut dest,
            &mut buf,
        )?;

        let mut audio_ix = 0;
        let mut video_ix = 0;
        while audio_ix < self.audio_tags.len() || video_ix < self.video_tags.len() {
            let (next_range, next_timestamp) = if video_ix >= self.video_tags.len() {
                let ret = &self.audio_tags[audio_ix];
                audio_ix += 1;
                (ret.range, ret.timestamp)
            } else if audio_ix >= self.audio_tags.len() {
                let ret = &self.video_tags[video_ix];
                video_ix += 1;
                (ret.range, ret.decode_timestamp)
            } else if self.audio_tags[audio_ix].timestamp
                < self.video_tags[video_ix].decode_timestamp
            {
                let ret = &self.audio_tags[audio_ix];
                audio_ix += 1;
                (ret.range, ret.timestamp)
            } else {
                let ret = &self.video_tags[video_ix];
                video_ix += 1;
                (ret.range, ret.decode_timestamp)
            };

            write_tag_with_timestamp(next_range, next_timestamp, &mut source, &mut dest, &mut buf)?;
        }

        write_tag_with_timestamp(
            self.video_end_of_sequence,
            self.end_of_sequence_timestamp,
            &mut source,
            &mut dest,
            &mut buf,
        )?;

        Ok(())
    }
}

fn read_audio_headers(mut inf: impl Read) -> io::Result<AacAudioInfo> {
    let audiodata = inf.read_u8()?;
    // All AAC data should have this header
    if audiodata != 0xAF {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported audio type: audio must be encoded as AAC-LC",
        ));
    }

    let ret = match inf.read_u8()? {
        0 => AacAudioInfo::SequenceHeader,
        1 => AacAudioInfo::Raw,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "corrupted input, invalid AACPacketType",
            ))
        }
    };

    Ok(ret)
}

fn read_video_headers(mut inf: impl Read) -> io::Result<AvcVideoInfo> {
    let seekable = match inf.read_u8()? {
        // (frame type 1, seekable)(data type 7, avc)
        0x17 => true,
        // (frame type 2, non-seekable)(data type 7, avc)
        0x27 => false,
        bad => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported video type: video must be encoded as h264 / AVC (type was {:x})",
                    bad
                ),
            ))
        }
    };

    let ret = match inf.read_u8()? {
        0 => AvcVideoInfo::SequenceHeader,
        2 => AvcVideoInfo::EndOfSequence,
        1 => {
            let composition_time_offset = inf.read_i24::<BigEndian>()?;
            AvcVideoInfo::Nalu {
                seekable,
                composition_time_offset,
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "corrupted input, unrecognized AVCPacketType",
            ))
        }
    };

    Ok(ret)
}

// Can't (apparently) use read_exact because we want to know about eofs, too.
fn read_ignore_interrupted(mut inf: impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    let mut b = buf;
    while !b.is_empty() {
        match inf.read(&mut b) {
            Ok(0) => break, // EOF
            Ok(n) => {
                read += n;
                b = &mut b[n..];
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    Ok(read)
}

fn read_tag_header(inf: &mut impl Read) -> io::Result<TagHeader> {
    let tagtype = inf.read_u8()?;
    let datasize = inf.read_u24::<BigEndian>()?;
    let low_decode_ts = inf.read_u24::<BigEndian>()?;
    let high_decode_ts = inf.read_u8()?;
    let stream_id = inf.read_u24::<BigEndian>()?;
    if stream_id != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "corrupted input, stream id != 0",
        ));
    }

    let decode_ts = (((high_decode_ts as u32) << 24) | low_decode_ts) as i32;

    Ok(TagHeader {
        datasize,
        tagtype,
        decode_ts,
    })
}

fn scan_tags<T: Read + Seek>(mut inf: T) -> io::Result<SeekMap> {
    // FLV header is 9 bytes, followed by 4 bytes of 0u32 for previous tag size, before the first tag.
    let mut offset = 9u64;
    let mut expect_previous_size = 0u32;
    let mut audio_tags = Vec::new();
    let mut video_tags = Vec::new();
    let mut audio_sequence_header = None;
    let mut video_sequence_header = None;
    let mut video_end_of_sequence = None;
    let mut end_of_sequence_timestamp = 0;

    // 4 bytes of previous size check
    // 11 bytes of header for next chunk
    // union(2 bytes of audio header, 5 bytes of video headers, 0 bytes of don't care.)
    let mut separator_buf: [u8; 21] = [0; 21];
    let tag_header_length = 11u32;
    let separator_length = tag_header_length + 4;
    loop {
        inf.seek(SeekFrom::Start(offset))?;

        let eof = match read_ignore_interrupted(&mut inf, &mut separator_buf[..]) {
            // 15 bytes is 4 bytes of size check + 11 bytes of tag header
            Ok(len) if len >= separator_length as usize => false,
            // 4 bytes of size check and EOF is a clean end to the file.
            Ok(len) if len == 4 => true,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "corrupted input, eof didn't line up with a previous size check",
                ));
            }
            Err(e) => return Err(e),
        };

        let mut reader = Cursor::new(separator_buf);
        let check_previous_size = reader.read_u32::<BigEndian>()?;
        if expect_previous_size != check_previous_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "corrupted input, expected size check didn't match",
            ));
        }

        if eof {
            return Ok(SeekMap {
                audio_sequence_header: audio_sequence_header.unwrap(),
                video_sequence_header: video_sequence_header.unwrap(),
                video_end_of_sequence: video_end_of_sequence.unwrap(),
                end_of_sequence_timestamp,
                audio_tags,
                video_tags,
            });
        }

        let tag_header = read_tag_header(&mut reader)?;

        // Size of the whole tag is size of the separator (minus the previous size check)
        // plus the size of the tag data payload.
        expect_previous_size = tag_header_length + tag_header.datasize;
        let tag_range = FileRange {
            offset: offset + 4, // don't count the Previous Size
            length: expect_previous_size,
        };

        match tag_header.tagtype {
            8 => match read_audio_headers(&mut reader)? {
                AacAudioInfo::SequenceHeader => {
                    audio_sequence_header = Some(tag_range);
                }
                AacAudioInfo::Raw => {
                    audio_tags.push(AudioTag {
                        range: tag_range,
                        timestamp: tag_header.decode_ts,
                    });
                }
            },
            9 => match read_video_headers(&mut reader)? {
                AvcVideoInfo::SequenceHeader => {
                    video_sequence_header = Some(tag_range);
                }
                AvcVideoInfo::Nalu {
                    seekable,
                    composition_time_offset,
                } => video_tags.push(VideoNaluTag {
                    range: tag_range,
                    decode_timestamp: tag_header.decode_ts,
                    composition_time_offset,
                    seekable,
                }),
                AvcVideoInfo::EndOfSequence => {
                    video_end_of_sequence = Some(tag_range);
                    end_of_sequence_timestamp = tag_header.decode_ts;
                }
            },
            18 => {
                // SCRIPTDATA tag, we ignore these.
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "corrupted input, invalid FLV tag type",
                ))
            }
        };

        offset += (tag_header.datasize + separator_length) as u64;
    }
}

const MIN_SLICE_INTERVAL: i32 = 10 * 1000; // 30 seconds in millis

fn shuffle_audio<R: Rng>(tags: &SeekMap, rng: &mut R) -> Vec<AudioTag> {
    if tags.audio_tags.is_empty() {
        return Vec::new();
    }

    let mut intervals = Vec::new(); // We could guess the capacity here if it matters...
    let original_timestamps: Vec<i32> = tags.audio_tags.iter().map(|tag| tag.timestamp).collect();
    let mut begin: usize = 0;
    let mut begin_ts: i32 = 0;
    for (ix, tag) in tags.audio_tags.iter().enumerate() {
        if (tag.timestamp - begin_ts) > MIN_SLICE_INTERVAL {
            intervals.push(begin..ix);
            begin = ix;
            begin_ts = tag.timestamp;
        }
    }

    let last_tag = match intervals.last() {
        None => Some(begin..tags.audio_tags.len()),
        Some(range) => {
            if range.end < tags.audio_tags.len() {
                Some(range.end..tags.audio_tags.len())
            } else {
                None
            }
        }
    };

    if let Some(range) = last_tag {
        intervals.push(range);
    }

    intervals.shuffle(rng);
    let mut ret = Vec::with_capacity(tags.audio_tags.len());
    for chunk in intervals {
        ret.extend(tags.audio_tags[chunk].iter().copied());
    }

    for (i, ts) in original_timestamps.iter().enumerate() {
        ret[i].timestamp = *ts;
    }

    ret
}

fn main() {
    let args = env::args();
    let infiles = args.skip(1).collect::<Vec<String>>();

    if infiles.len() != 1 {
        panic!("provide exactly one flv filename as an argument");
    }

    let fname = infiles.first().unwrap();
    let file = File::open(fname).unwrap();
    let mut tags = scan_tags(&file).unwrap();

    let mut rng = rand::thread_rng();
    tags.audio_tags = shuffle_audio(&tags, &mut rng);

    tags.dump(&file, std::io::stdout()).unwrap();
}
