use byteorder::{BigEndian, WriteBytesExt};
use std::io;
use std::io::Write;
use x264::{Encoder, Param, Picture};

pub trait Show {
    fn frame(self, frame: usize, picture: &mut Picture) -> Self;
}

pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 720;
pub const DEFAULT_FRAME_RATE: usize = 30; // in fps

// From https://www.adobe.com/content/dam/acom/en/devnet/flv/video_file_format_spec_v10.pdf
const FLV_HEADER: [u8; 9] = [
    0x46, 0x4c, 0x56, // 'FLV'
    0x01, // version 1
    0x05, // use video and audio
    0x0, 0x0, 0x0,  // reserved
    0x09, // size of this header
];

pub fn streaming_params(fps: usize) -> Param {
    let param = Param::default_preset("veryfast", None).unwrap();
    let param = param.set_dimension(HEIGHT, WIDTH);

    // x264-rs doesn't seem to have a way to set the color space
    // (since param.par.i_csp is private, and there isn't [apparently]
    // a param_parse trick to set the color space.)
    // So we're assuming that we're in i420 color space.
    let framerate_s = format!("{}", fps);

    let param = param.param_parse("fps", &framerate_s).unwrap();
    let param = param.param_parse("repeat_headers", "1").unwrap();
    let param = param.param_parse("keyint", &framerate_s).unwrap();
    param.apply_profile("high").unwrap()
}

/// duration is in number of frames
pub fn stream(show: impl Show, duration: Option<usize>, fps: Option<usize>) {
    let target_fps = fps.unwrap_or(DEFAULT_FRAME_RATE);
    let mut param = streaming_params(target_fps);
    let mut picture = Picture::from_param(&param).unwrap();
    let mut encoder = Encoder::open(&mut param).unwrap();
    let mut previous_tag_size = 0u32;

    let mut i = 0usize;
    let mut show = show;

    // TODO blocking writes on stdout is probably the wrong thing
    // unless you know you're way out ahead of the stream.
    // Might be worth checking out tokio and manually buffering
    // video output internally? Or maybe there is a nice
    // buffered output we can use?

    io::stdout().write_all(&FLV_HEADER).unwrap();
    io::stdout()
        .write_u32::<BigEndian>(previous_tag_size)
        .unwrap();

    let h264_headers = encoder.get_headers().unwrap().as_bytes();
    let header_packet_length = h264_headers.len() as u32 + 4;

    // TODO it'd be much more polite to write all of this header
    // stuff into a buffer.
    io::stdout().write_u8(0x09).unwrap(); // VIDEO
    io::stdout()
        .write_u24::<BigEndian>(header_packet_length)
        .unwrap();
    io::stdout()
        .write_u24::<BigEndian>(0x0) // TIMESTAMP
        .unwrap();
    io::stdout()
        .write_u8(0x0) // EXTENDED TIMESTAMP
        .unwrap();
    io::stdout().write_u24::<BigEndian>(0x0).unwrap(); // Stream ID

    while duration.is_none() || duration.unwrap() > i {
        show = show.frame(i, &mut picture);

        picture = picture.set_timestamp(i as i64);
        if let Some((nal, presentation_ts, decoding_ts)) = encoder.encode(&picture).unwrap() {
            let buf = nal.as_bytes();

            // Per some stack overflow type
            // pts and dts are in 1/90,000 of a second.
            // The "timestamp" field in the FLV is the DECODE timestamp
            // of the video, so:
            let timestamp = decoding_ts / 90;

            // "composition time", in the data packet, is the difference in millis
            // between decoding time and presentation time, so:
            let composition_time_offset = (presentation_ts - decoding_ts) / 90;

            // 4 bytes of additional data in the video packet, the
            // AVCVIDEO packet type and the composition_time_offset
            let packet_length = buf.len() as u32 + 4;

            // TODO it'd be much more polite to write all of this header
            // stuff into a buffer.
            io::stdout().write_u8(0x09).unwrap(); // VIDEO
            io::stdout().write_u24::<BigEndian>(packet_length).unwrap();
            io::stdout()
                .write_u24::<BigEndian>((timestamp & 0xffffff) as u32)
                .unwrap();
            io::stdout()
                .write_u8((timestamp >> 24 & 0xff) as u8)
                .unwrap();
            io::stdout().write_u24::<BigEndian>(0x0).unwrap(); // Stream ID

            io::stdout().write_u8(0x01).unwrap(); // AVCVIDEO packet type 1, NALU
            io::stdout()
                .write_i24::<BigEndian>(composition_time_offset as i32)
                .unwrap();

            io::stdout().write_all(buf).unwrap();
            previous_tag_size = packet_length + 11;
            io::stdout()
                .write_u32::<BigEndian>(previous_tag_size)
                .unwrap();
        }
        i += 1;
    }

    while encoder.delayed_frames() {
        if let Some((nal, _, _)) = encoder.encode(None).unwrap() {
            let buf = nal.as_bytes();
            io::stdout().write_all(buf).unwrap();
        }
    }
}
