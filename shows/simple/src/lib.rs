use std::io;
use std::io::Write;
use x264::{Encoder, Param, Picture};

pub trait Show {
    fn frame(self, frame: usize, picture: &mut Picture) -> Self;
}

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const FRAME_RATE: usize = 30; // in fps

/// duration is in number of frames
pub fn stream(show: impl Show, duration: Option<usize>) {
    let param = Param::default_preset("veryfast", None).unwrap();
    let param = param.set_dimension(HEIGHT, WIDTH);

    // x264-rs doesn't seem to have a way to set the color space
    // (since param.par.i_csp is private, and there isn't [apparently]
    // a param_parse trick to set the color space.)
    // So we're assuming that we're in i420 color space.
    let framerate_s = format!("{}", FRAME_RATE);

    let param = param.param_parse("fps", &framerate_s).unwrap();
    let param = param.param_parse("repeat_headers", "1").unwrap();
    let param = param.param_parse("annexb", "1").unwrap();
    let param = param.param_parse("keyint", &framerate_s).unwrap();
    let mut param = param.apply_profile("high").unwrap();

    let mut picture = Picture::from_param(&param).unwrap();
    let mut encoder = Encoder::open(&mut param).unwrap();

    set_constant(128, picture.as_mut_slice(1).unwrap());
    set_constant(128, picture.as_mut_slice(2).unwrap());

    let mut i = 0usize;
    let mut show = show;
    while duration.is_none() || duration.unwrap() > i {
        show = show.frame(i, &mut picture);

        picture = picture.set_timestamp(i as i64);
        if let Some((nal, _, _)) = encoder.encode(&picture).unwrap() {
            let buf = nal.as_bytes();

            // TODO blocking on stdout is probably the wrong thing
            // unless you know you're way out ahead of the stream.
            // Might be worth checking out tokio and manually buffering
            // video output internally? Or maybe there is a nice
            // buffered output we can use?
            io::stdout().write_all(buf).unwrap();
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

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}
