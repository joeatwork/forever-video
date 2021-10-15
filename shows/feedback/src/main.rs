use rand;
use simple;
use std::io;
use std::io::Write;
use x264::{Encoder, Picture};

// Not a very good show
// 1) h264 compression is very good, and so there isn't much of it per frame.
// 2) h264 compression is very good, so it looks a lot like random noise.
fn main() {
    let mut param = simple::streaming_params();
    let mut picture = Picture::from_param(&param).unwrap();
    let mut encoder = Encoder::open(&mut param).unwrap();

    for u in picture.as_mut_slice(1).unwrap() {
        *u = 128;
    }

    for v in picture.as_mut_slice(2).unwrap() {
        *v = 128;
    }

    for i in 0..300 {
        picture = picture.set_timestamp(i as i64);
        let mut needs_noise = true;
        if let Some((nal, _, _)) = encoder.encode(&picture).unwrap() {
            let buf = nal.as_bytes();
            io::stdout().write_all(buf).unwrap();
            if buf.len() > 100 {
                frame(buf, &mut picture);
                needs_noise = false;
            }
        };

        if needs_noise {
            for y in picture.as_mut_slice(0).unwrap() {
                *y = rand::random();
            }
        }
    }

    while encoder.delayed_frames() {
        if let Some((nal, _, _)) = encoder.encode(None).unwrap() {
            let buf = nal.as_bytes();
            io::stdout().write_all(buf).unwrap();
        }
    }
}

fn frame(buf: &[u8], picture: &mut Picture) {
    assert_ne!(buf.len(), 0);

    let y_plane = picture.as_mut_slice(0).unwrap();
    assert_eq!(y_plane.len(), simple::WIDTH * simple::HEIGHT);

    let scale = (buf.len() as f64) / (y_plane.len() as f64);

    let swidth = ((simple::WIDTH as f64) * scale).floor() as usize;
    let sheight = ((simple::HEIGHT as f64) * scale).floor() as usize;

    assert!(swidth * sheight <= buf.len());

    eprintln!("TODO Buffer len {}", buf.len());

    if scale < 1.0 {
        // buf is SMALLER than image, so scale it
        let off_width = (simple::WIDTH - swidth) / 2;
        let off_height = (simple::HEIGHT - sheight) / 2;

        for src_y in 0..sheight {
            let dest_y = src_y + off_height;
            for src_x in 0..swidth {
                let dest_x = src_x + off_width;
                y_plane[simple::WIDTH * dest_y + dest_x] = buf[sheight * src_y + src_x];
            }
        }
    } else {
        // buf is LARGER than image, so crop it
        let off_width = (swidth - simple::WIDTH) / 2;
        let off_height = (sheight - simple::HEIGHT) / 2;

        for dest_y in 0..simple::HEIGHT {
            let src_y = dest_y + off_height;
            for dest_x in 0..simple::WIDTH {
                let src_x = dest_x + off_width;
                y_plane[simple::WIDTH * dest_y + dest_x] = buf[sheight * src_y + src_x];
            }
        }
    }
}
