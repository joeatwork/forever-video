use simple::Show;
use std::env;
use x264::Picture;

const SIN_AT_FRAME: [u8; 60] = [
    128, 141, 154, 167, 179, 191, 202, 213, 222, 231, 238, 244, 249, 252, 254, 255, 254, 252, 249,
    244, 238, 231, 222, 213, 202, 191, 179, 167, 154, 141, 128, 114, 101, 88, 76, 64, 53, 42, 33,
    24, 17, 11, 6, 3, 1, 0, 1, 3, 6, 11, 17, 24, 33, 42, 53, 64, 76, 88, 101, 114,
];

struct SimpleShow {}

impl Show for SimpleShow {
    fn frame(self, frame: usize, picture: &mut Picture) -> Self {
        if frame == 0 {
            set_constant(128, picture.as_mut_slice(1).unwrap());
            set_constant(128, picture.as_mut_slice(2).unwrap());
        }

        let ix = frame % SIN_AT_FRAME.len();
        let lum = SIN_AT_FRAME[ix];
        let buf = picture.as_mut_slice(0).unwrap();
        for x in buf {
            *x = lum;
        }

        self
    }
}

fn main() {
    let mut args = env::args();
    let duration = if args.len() > 1 {
        let d = args.nth(1).unwrap();
        Some(d.parse::<usize>().unwrap())
    } else {
        None
    };

    simple::stream(SimpleShow {}, duration, None);
}

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}
