use byteorder::{BigEndian, WriteBytesExt};
use std::ffi::CString;
use std::io;
use std::io::Write;
use std::mem;
use std::os::raw;
use std::ptr;
use std::slice;

pub trait Show {
    fn frame(self, frame: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) -> Self;
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

fn stream_params(fps: u32) -> x264_sys::x264_param_t {
    let mut param: mem::MaybeUninit<x264_sys::x264_param_t> = mem::MaybeUninit::uninit();
    let veryfast = CString::new("veryfast").unwrap();
    let mut param = match unsafe {
        x264_sys::x264_param_default_preset(
            param.as_mut_ptr(),
            veryfast.as_ptr() as *const i8,
            ptr::null(),
        )
    } {
        0 => unsafe { param.assume_init() },
        _ => unreachable!(),
    };

    param.i_fps_num = fps;
    param.i_fps_den = 1;
    param.i_keyint_max = 30;
    param.i_keyint_min = 0;
    param.i_height = HEIGHT as i32;
    param.i_width = WIDTH as i32;

    let high = CString::new("high").unwrap();

    match unsafe { x264_sys::x264_param_apply_profile(&mut param, high.as_ptr() as *const i8) } {
        0 => param,
        _ => unreachable!(),
    }
}

struct Picture {
    picture: x264_sys::x264_picture_t,
}

impl Picture {
    fn new(param: &x264_sys::x264_param_t) -> Self {
        let mut picture: mem::MaybeUninit<x264_sys::x264_picture_t> = mem::MaybeUninit::uninit();
        let picture = match unsafe {
            x264_sys::x264_picture_alloc(
                picture.as_mut_ptr(),
                param.i_csp,
                param.i_width,
                param.i_height,
            )
        } {
            0 => unsafe { picture.assume_init() },
            _ => panic!("allocation failure"),
        };

        Picture { picture }
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        unsafe { x264_sys::x264_picture_clean(&mut self.picture as *mut x264_sys::x264_picture_t) }
    }
}

struct Encoder {
    encoder: *mut x264_sys::x264_t,
}

impl Encoder {
    fn new(param: &mut x264_sys::x264_param_t) -> Self {
        let encoder = unsafe { x264_sys::x264_encoder_open(param as *mut x264_sys::x264_param_t) };

        if encoder.is_null() {
            panic!("allocation failure");
        }

        Encoder { encoder }
    }

    fn encode_picture(&mut self, pic_in: *const x264_sys::x264_picture_t) -> Vec<u8> {
        let mut pic_out: mem::MaybeUninit<x264_sys::x264_picture_t> = mem::MaybeUninit::uninit();
        let mut pp_nal: mem::MaybeUninit<*mut x264_sys::x264_nal_t> = mem::MaybeUninit::uninit();
        let mut pi_nal: raw::c_int = 0;

        let result = unsafe {
            x264_sys::x264_encoder_encode(
                self.encoder,
                pp_nal.as_mut_ptr(),
                &mut pi_nal as *mut raw::c_int,
                pic_in as *mut x264_sys::x264_picture_t,
                pic_out.as_mut_ptr(),
            )
        };

        if result < 0 {
            panic!("can't encode"); // You will regret this.
        }

        let _pic_out = unsafe { pic_out.assume_init() };
        let pp_nal = unsafe { pp_nal.assume_init() };
        let mut encoded: Vec<u8> = Vec::new();

        // TODO I'm unsure that smooshing these nals together is legit, if I
        // (for example) need to label a particular NAU as a seekable keyframe
        // in the output...
        for i in 0..pi_nal {
            let nal = unsafe { Box::from_raw(pp_nal.offset(i as isize)) };
            let payload = unsafe { slice::from_raw_parts(nal.p_payload, nal.i_payload as usize) };

            encoded.extend_from_slice(payload);

            mem::forget(nal);
        }

        encoded
    }

    fn delayed_frames(&mut self) -> usize {
        let ret = unsafe { x264_sys::x264_encoder_delayed_frames(self.encoder) };
        ret as usize
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { x264_sys::x264_encoder_close(self.encoder) };
    }
}

/// duration is in number of frames
pub fn stream(show: impl Show, duration: Option<usize>, fps: Option<usize>) {
    let framerate = fps.unwrap_or(DEFAULT_FRAME_RATE);
    let mut param = stream_params(framerate as u32);
    let mut picture = Picture::new(&param);
    let mut encoder = Encoder::new(&mut param);

    THIS IS PRETTY MUCH NONSENSE
    NEXT STEPS ARE TO EMIT FLV STUFF.

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

    // TODO need to write get_headers here so you can actually get the headers.

    let h264_headers = encoder.get_headers().unwrap().as_bytes();
    let header_packet_length = h264_headers.len() as u32 + 5;

    // TODO it'd be much more polite to write all of this header
    // stuff into a buffer.

    // TAG HEADER
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

    // VIDEODATA HEADER
    io::stdout().write_u8(0x27).unwrap(); // [0010] "keyframe", [0111] "avc codec"

    // AVCVIDEODATA HEADER
    io::stdout().write_u8(0x0).unwrap(); // header
    io::stdout().write_i24::<BigEndian>(0x0).unwrap(); // composition time, zero

    io::stdout().write_all(h264_headers);

    // TODO need decoding_ts, presentation_ts, and Seekable-ness from encoder.

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

            // SHOWSTOPPER! We need to know whether the given NAU is a keyframe
            // or not. This data is *not* provided by x264-rs. So we need to
            // go ahead and use bindgen

            //
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
    // h264 time in 90,000 ticks per second, framerate in frames / second
    let ticks_per_frame = 90000 / framerate as i64;
    while duration.is_none() || duration.unwrap() > i {
        let y_plane =
            unsafe { slice::from_raw_parts_mut(picture.picture.img.plane[0], WIDTH * HEIGHT) };
        let u_plane = unsafe {
            std::slice::from_raw_parts_mut(picture.picture.img.plane[1], (WIDTH * HEIGHT) >> 2)
        };
        let v_plane = unsafe {
            std::slice::from_raw_parts_mut(picture.picture.img.plane[2], (WIDTH * HEIGHT) >> 2)
        };

        show = show.frame(i, y_plane, u_plane, v_plane);
        picture.picture.i_pts += ticks_per_frame;

        let buf = encoder.encode_picture(&picture.picture);

        // TODO blocking on stdout is probably the wrong thing
        // unless you know you're way out ahead of the stream.
        // Might be worth checking out tokio and manually buffering
        // video output internally? Or maybe there is a nice
        // buffered output we can use?
        io::stdout().write_all(&buf).unwrap();
        i += 1;
    }

    while encoder.delayed_frames() > 0 {
        let buf = encoder.encode_picture(ptr::null());
        io::stdout().write_all(&buf).unwrap();
    }
}
