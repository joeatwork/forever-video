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
    let framerate = fps.unwrap_or(30);
    let mut param = stream_params(framerate as u32);
    let mut picture = Picture::new(&param);
    let mut encoder = Encoder::new(&mut param);

    let mut i = 0usize;
    let mut show = show;

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
