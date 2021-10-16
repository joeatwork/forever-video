use simple;
use simple::Show;
use x264::Picture;

struct YUV {
    y: u8,
    u: u8,
    v: u8,
}

struct LightCycle {
    color: YUV,
    // x, y are in the uv plane, NOT the y plane
    x: usize,
    y: usize,
}

const UV_WIDTH: usize = simple::WIDTH >> 1;
const UV_HEIGHT: usize = simple::HEIGHT >> 1;

impl LightCycle {
    #[inline]
    fn uv_index(&self) -> usize {
        (UV_WIDTH * self.y) + self.x
    }

    #[inline]
    fn y_indexes(&self) -> [usize; 4] {
        let y = self.y * 2;
        let x = self.x * 2;
        let row1 = (simple::WIDTH * y) + x;
        let row2 = (simple::WIDTH * (y + 1)) + x;
        [row1, row1 + 1, row2, row2 + 1]
    }
}

struct LightCycleShow {
    cycles: Vec<LightCycle>,
}

impl Show for LightCycleShow {
    fn frame(mut self, frame: usize, picture: &mut Picture) -> Self {
        if frame == 0 {
            set_constant(0, picture.as_mut_slice(0).unwrap());
            set_constant(128, picture.as_mut_slice(1).unwrap());
            set_constant(128, picture.as_mut_slice(2).unwrap());
        }

        let y_plane = picture.as_mut_slice(0).unwrap();
        for cycle in &self.cycles {
            for ix in cycle.y_indexes() {
                y_plane[ix] = cycle.color.y;
            }
        }

        let u_plane = picture.as_mut_slice(1).unwrap();
        for cycle in &self.cycles {
            u_plane[cycle.uv_index()] = cycle.color.u;
        }

        let v_plane = picture.as_mut_slice(2).unwrap();
        for cycle in &self.cycles {
            v_plane[cycle.uv_index()] = cycle.color.v;
        }

        for cycle in &mut self.cycles {
            cycle.x = (cycle.x + 1) % UV_WIDTH;
        }

        self
    }
}

fn main() {
    let show = LightCycleShow {
        cycles: vec![LightCycle {
            color: YUV {
                y: 255,
                u: 255,
                v: 0,
            },
            x: UV_WIDTH / 2,
            y: UV_HEIGHT / 2,
        }],
    };
    simple::stream(show, Some(300));
}

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}
