use byteorder::{BigEndian, WriteBytesExt};
use std::convert::TryFrom;
use std::io::{self, Write};

// From https://www.adobe.com/content/dam/acom/en/devnet/flv/video_file_format_spec_v10.pdf
const FLV_HEADER: [u8; 9] = [
    0x46, 0x4c, 0x56, // 'FLV'
    0x01, // version 1
    0x05, // use video and audio
    0x0, 0x0, 0x0,  // reserved
    0x09, // size of this header
];

pub enum AvcPacketType {
    SequenceHeader { data: Vec<u8> },
    Nalu { presentation_ts: i64, data: Vec<u8> },
    SequenceEnd,
}

pub fn write_flv_header(out: &mut impl Write) -> io::Result<()> {
    out.write_all(&FLV_HEADER)?;
    out.write_u32::<BigEndian>(0)?; // previous tag size is zero
    Ok(())
}

pub enum MediaType {
    Audio = 8,
    Video = 9,
}

pub fn write_media_tag_header(
    out: &mut impl Write,
    media_type: MediaType,
    data_size: u32,
    decode_timestamp: i32,
) -> io::Result<()> {
    out.write_u8(media_type as u8)?; // tag type - 9 == video
    out.write_u24::<BigEndian>(data_size)?;
    out.write_u24::<BigEndian>((decode_timestamp & 0xffffff) as u32)?;
    out.write_u8((decode_timestamp >> 24 & 0xff) as u8)?;
    out.write_u24::<BigEndian>(0x0)?; // stream id

    Ok(())
}

// Writes 11 bytes of tag header
pub fn write_video_tag_header(
    out: &mut impl Write,
    data_size: u32,
    decode_timestamp: i32,
) -> io::Result<()> {
    write_media_tag_header(out, MediaType::Video, data_size, decode_timestamp)
}

// Writes 11 bytes of tag header
pub fn write_audio_tag_header(
    out: &mut impl Write,
    data_size: u32,
    decode_timestamp: i32,
) -> io::Result<()> {
    write_media_tag_header(out, MediaType::Audio, data_size, decode_timestamp)
}

/// input timestamps should be in h264 ticks, 1/90,000 of a second.
pub fn write_video_tag(
    mut out: &mut impl Write,
    decode_ts: i64,
    seekable: bool,
    packet_type: AvcPacketType,
) -> io::Result<()> {
    let (packet_type_code, presentation_ts, data) = match packet_type {
        AvcPacketType::SequenceHeader { data } => (0, 0, data),
        AvcPacketType::SequenceEnd => (2, 0, vec![]),
        AvcPacketType::Nalu {
            presentation_ts: ts,
            data,
        } => (1, ts, data),
    };

    // Data length is data.len() + 1 byte videodata header + 4 bytes avcvideopacket header
    let data_size = u32::try_from(data.len()).unwrap() + 1 + 4;

    let decode_millis = decode_ts / 90;
    let composition_offset_millis = i32::try_from((presentation_ts - decode_ts) / 90).unwrap();

    // Tag header - 11 bytes
    write_video_tag_header(&mut out, data_size, i32::try_from(decode_millis).unwrap())?;

    // VIDEODATA header - one byte
    let frametype = if seekable { 1u8 << 4 } else { 2u8 << 4 };
    let codec_id = 7u8; // AVC codec
    out.write_u8(frametype | codec_id)?;

    // AVCVIDEOPACKET header - 4 bytes
    out.write_u8(packet_type_code)?;
    out.write_i24::<BigEndian>(composition_offset_millis)?;

    out.write_all(&data)?;

    // Total tag length is data_size + 11 bytes tag header
    out.write_u32::<BigEndian>(data_size + 11)?;

    Ok(())
}
