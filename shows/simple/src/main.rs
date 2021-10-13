use std::io;
use std::io::Write;

use x264::{Encoder, Param, Picture};

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const DURATION: usize = 300; // in FRAMES
const FRAME_RATE: usize = 30; // in fps

const SIN_AT_FRAME: [u8; 60] = [
    128, 141, 154, 167, 179, 191, 202, 213, 222, 231, 238, 244, 249, 252, 254, 255, 254, 252, 249,
    244, 238, 231, 222, 213, 202, 191, 179, 167, 154, 141, 128, 114, 101, 88, 76, 64, 53, 42, 33,
    24, 17, 11, 6, 3, 1, 0, 1, 3, 6, 11, 17, 24, 33, 42, 53, 64, 76, 88, 101, 114,
];

fn main() {
    let param = Param::default_preset("veryfast", None).unwrap();
    let param = param.set_dimension(HEIGHT, WIDTH);

    // Formats in https://wiki.videolan.org/YUV cross referenced with
    // $ x264 --fullhelp
    //
    // let param = param.param_parse("csp", "i420").unwrap();
    // x264-rs doesn't seem to have a way to set the color space
    // (since param.par.i_csp is private, and there isn't [apparently]
    // a param_parse trick to set the color space.)

    let param = param
        .param_parse("fps", &format!("{}", FRAME_RATE))
        .unwrap();
    let param = param.param_parse("repeat_headers", "1").unwrap();
    let param = param.param_parse("annexb", "1").unwrap();
    let mut param = param.apply_profile("high").unwrap();

    // TODO there is a *lot* that can go down with the params here.

    let mut picture = Picture::from_param(&param).unwrap();
    let mut encoder = Encoder::open(&mut param).unwrap();

    set_constant(128, picture.as_mut_slice(1).unwrap());
    set_constant(128, picture.as_mut_slice(2).unwrap());

    for i in 0..DURATION {
        frame(i, &mut picture);

        picture = picture.set_timestamp(i as i64);
        if let Some((nal, _, _)) = encoder.encode(&picture).unwrap() {
            let buf = nal.as_bytes();
            io::stdout().write_all(buf).unwrap();
        }
    }

    while encoder.delayed_frames() {
        if let Some((nal, _, _)) = encoder.encode(None).unwrap() {
            let buf = nal.as_bytes();
            io::stdout().write_all(buf).unwrap();
        }
    }
}

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}

fn frame(frame: usize, picture: &mut Picture) {
    let ix = frame % SIN_AT_FRAME.len();
    let lum = SIN_AT_FRAME[ix];
    let buf = picture.as_mut_slice(0).unwrap();
    for x in buf {
        *x = lum;
    }
}
