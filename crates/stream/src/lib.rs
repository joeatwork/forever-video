use std::cmp;
use std::ffi::CString;
use std::io;
use std::mem;
use std::os::raw;
use std::ptr;
use std::slice;

use flvmux::{write_flv_header, write_video_tag, AvcPacketType};

use libx264_sys::*;

pub trait Show {
    fn frame(self, frame: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) -> Self;
}

pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 720;
pub const DEFAULT_FRAME_RATE: u32 = 30; // in fps

fn stream_params(fps: u32) -> x264_param_t {
    let mut param: mem::MaybeUninit<x264_param_t> = mem::MaybeUninit::uninit();
    let veryfast = CString::new("veryfast").unwrap();
    let mut param = match unsafe {
        x264_param_default_preset(
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

    match unsafe { x264_param_apply_profile(&mut param, high.as_ptr() as *const i8) } {
        0 => param,
        _ => unreachable!(),
    }
}

struct Picture {
    picture: x264_picture_t,
}

impl Picture {
    fn new(param: &x264_param_t) -> Self {
        let mut picture: mem::MaybeUninit<x264_picture_t> = mem::MaybeUninit::uninit();
        let picture = match unsafe {
            x264_picture_alloc(
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
        unsafe { x264_picture_clean(&mut self.picture as *mut x264_picture_t) }
    }
}

struct Encoder {
    encoder: *mut x264_t,
}

struct Encoded {
    data: Vec<u8>,
    seekable: bool,
    presentation_ts: i64,
    decode_ts: i64,
}

impl Encoder {
    fn new(param: &mut x264_param_t) -> Self {
        // libx264 defines "x264_encode_open" as a macro, that expands to
        // another function name that knows the build version. If you change
        // the version of the lib to (say) 999, you'll need to change the line
        // below to x264_encoder_open_999
        let encoder = unsafe { x264_encoder_open_155(param as *mut x264_param_t) };

        if encoder.is_null() {
            panic!("allocation failure");
        }

        Encoder { encoder }
    }

    fn headers(&mut self) -> Vec<u8> {
        let mut pp_nal: mem::MaybeUninit<*mut x264_nal_t> = mem::MaybeUninit::uninit();
        let mut pi_nal: raw::c_int = 0;

        let ret = unsafe {
            x264_encoder_headers(
                self.encoder,
                pp_nal.as_mut_ptr(),
                &mut pi_nal as *mut raw::c_int,
            )
        };

        if ret < 0 {
            panic!("can't produce encoder headers");
        }

        let pp_nal = unsafe { pp_nal.assume_init() };
        let mut data = Vec::new();
        for i in 0..pi_nal {
            let nal = unsafe { Box::from_raw(pp_nal.offset(i as isize)) };
            let payload = unsafe { slice::from_raw_parts(nal.p_payload, nal.i_payload as usize) };
            data.extend_from_slice(payload);

            mem::forget(nal);
        }

        data
    }

    fn encode_picture(&mut self, pic_in: Option<&mut x264_picture_t>) -> Option<Encoded> {
        let mut pic_out: mem::MaybeUninit<x264_picture_t> = mem::MaybeUninit::uninit();
        let mut pp_nal: mem::MaybeUninit<*mut x264_nal_t> = mem::MaybeUninit::uninit();
        let mut pi_nal: raw::c_int = 0;

        let pic_in_ptr = match pic_in {
            Some(p) => p as *mut x264_picture_t,
            None => ptr::null::<*const x264_picture_t>() as *mut x264_picture_t,
        };

        let result = unsafe {
            x264_encoder_encode(
                self.encoder,
                pp_nal.as_mut_ptr(),
                &mut pi_nal as *mut raw::c_int,
                pic_in_ptr,
                pic_out.as_mut_ptr(),
            )
        };

        if result < 0 {
            panic!("can't encode");
        }

        if pi_nal <= 0 {
            return None;
        }

        let pic_out = unsafe { pic_out.assume_init() };
        let pp_nal = unsafe { pp_nal.assume_init() };
        let mut data = Vec::new();
        let mut seekable = false;

        // OK, we have an array of nal units, and *some* of them might be IDR frames?
        for i in 0..pi_nal {
            let nal = unsafe { Box::from_raw(pp_nal.offset(i as isize)) };

            // I *believe* that if we have any seekable nal units, we'll have ONLY
            // the one seekable nal unit.
            seekable = seekable || nal.i_type == nal_unit_type_e_NAL_SLICE_IDR as i32;
            let payload = unsafe { slice::from_raw_parts(nal.p_payload, nal.i_payload as usize) };

            data.extend_from_slice(payload);
            mem::forget(nal);
        }

        Some(Encoded {
            data,
            seekable,
            decode_ts: pic_out.i_dts,
            presentation_ts: pic_out.i_pts,
        })
    }

    fn delayed_frames(&mut self) -> i32 {
        unsafe { x264_encoder_delayed_frames(self.encoder) }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { x264_encoder_close(self.encoder) };
    }
}

/// duration is in number of frames
pub fn stream(show: impl Show, duration: Option<usize>, fps: Option<u32>) {
    let framerate = fps.unwrap_or(DEFAULT_FRAME_RATE);
    let mut param = stream_params(framerate);
    let mut picture = Picture::new(&param);
    let mut encoder = Encoder::new(&mut param);
    let mut show = show;

    // TODO blocking writes on stdout is probably the wrong thing
    // consider a buffered writer.
    let mut out = io::stdout();

    write_flv_header(&mut out).unwrap();

    let h264_headers = encoder.headers();
    write_video_tag(
        &mut out,
        0,
        true, // headers are apparently seekable
        AvcPacketType::SequenceHeader { data: h264_headers },
    )
    .unwrap();

    // h264 time in 90,000 ticks per second, framerate in frames / second
    let ticks_per_frame = 90000 / i64::from(framerate);
    let mut frame = 0usize;
    while duration.is_none() || duration.unwrap() > frame {
        let y_plane =
            unsafe { slice::from_raw_parts_mut(picture.picture.img.plane[0], WIDTH * HEIGHT) };
        let u_plane = unsafe {
            std::slice::from_raw_parts_mut(picture.picture.img.plane[1], (WIDTH * HEIGHT) >> 2)
        };
        let v_plane = unsafe {
            std::slice::from_raw_parts_mut(picture.picture.img.plane[2], (WIDTH * HEIGHT) >> 2)
        };

        show = show.frame(frame, y_plane, u_plane, v_plane);
        picture.picture.i_pts += ticks_per_frame;

        if let Some(encoded) = encoder.encode_picture(Some(&mut picture.picture)) {
            write_video_tag(
                &mut out,
                encoded.decode_ts,
                encoded.seekable,
                AvcPacketType::Nalu {
                    presentation_ts: encoded.presentation_ts,
                    data: encoded.data,
                },
            )
            .unwrap();
        }

        frame += 1;
    }

    let mut last_presentation_time = picture.picture.i_pts;
    while encoder.delayed_frames() > 0 {
        let encoded = encoder.encode_picture(None).unwrap();
        last_presentation_time = cmp::max(encoded.presentation_ts, last_presentation_time);
        write_video_tag(
            &mut out,
            encoded.decode_ts,
            encoded.seekable,
            AvcPacketType::Nalu {
                presentation_ts: encoded.presentation_ts,
                data: encoded.data,
            },
        )
        .unwrap();
    }

    // last_presentation_time and seekable here are best guesses.
    write_video_tag(
        &mut out,
        last_presentation_time,
        true, // Seekable? Sure, why not?
        AvcPacketType::SequenceEnd,
    )
    .unwrap();
}
