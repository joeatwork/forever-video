use byteorder::{BigEndian, ReadBytesExt};
use std::env;
use std::fs::File;
use std::io;
use std::io::Cursor;
use std::io::SeekFrom;
use std::io::{Read, Seek};

// THE PLAN - read an FLV, push out another FLV

// PHASE -1:

// [] reading the FLV
// [] storing as much of the structure as you wanna
// [] writing an FLV stream without stream(Show...)
//        Do we hafta write headers?

// PHASE 0:
// [] Shuffle audio tags.

// PHASE 0.5:
// [] Shuffle the video tags and see if it works, or just makes the stream crummy.
//    (It's POSSIBLE that this will "just work")

// PHASE 1:
// [] update h264 decode time / presentation times (which means being smart about the content)
//    and then shuffling the video tags.

#[derive(Debug)]
struct FileRange {
    offset: u64,
    length: u32,
}

/// Guide for finding a tag in an FLV file.
#[derive(Debug)]
struct TagInfo {
    /// Position of the tag in the file, not including any previous tag size
    range: FileRange,
    /// Presentation / Play time associated with the
    timestamp: i32,
}

#[derive(Debug)]
struct VideoTag(TagInfo);

#[derive(Debug)]
struct AudioTag(TagInfo);

#[derive(Debug)]
struct SeekMap {
    audio_sequence_header: FileRange,
    video_sequence_header: FileRange,
    video_end_of_sequence: FileRange,
    audio_tags: Vec<AudioTag>,
    video_tags: Vec<VideoTag>,
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
                    audio_tags.push(AudioTag(TagInfo {
                        range: tag_range,
                        timestamp: tag_header.decode_ts,
                    }));
                }
            },
            9 => match read_video_headers(&mut reader)? {
                AvcVideoInfo::SequenceHeader => {
                    video_sequence_header = Some(tag_range);
                }
                AvcVideoInfo::Nalu {
                    seekable,
                    composition_time_offset,
                } => {
                    if seekable {
                        video_tags.push(VideoTag(TagInfo {
                            range: tag_range,
                            timestamp: tag_header.decode_ts + composition_time_offset,
                        }))
                    }
                }
                AvcVideoInfo::EndOfSequence => {
                    video_end_of_sequence = Some(tag_range);
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

fn main() {
    let args = env::args();
    let infiles = args.skip(1).collect::<Vec<String>>();

    if infiles.len() != 1 {
        panic!("provide exactly one flv filename as an argument");
    }

    let fname = infiles.first().unwrap();
    let file = File::open(fname).unwrap();
    let tags = scan_tags(file).unwrap();

    println!("TAGS\n{:#?}", tags);
}
