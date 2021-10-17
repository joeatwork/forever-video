use simple::Show;
use x264::Picture;

mod line;

struct Yuv {
    y: u8,
    u: u8,
    v: u8,
}

struct LightCycle {
    color: Yuv,
    // x, y are in the uv plane, NOT the y plane
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

const UV_WIDTH: usize = simple::WIDTH >> 1;
const UV_HEIGHT: usize = simple::HEIGHT >> 1;

#[inline]
fn uv_index(x: usize, y: usize) -> usize {
    (UV_WIDTH * y) + x
}

#[inline]
fn y_indexes(x: usize, y: usize) -> [usize; 4] {
    let scaled_x = x * 2;
    let scaled_y = y * 2;
    let row1 = (simple::WIDTH * scaled_y) + scaled_x;
    let row2 = (simple::WIDTH * (scaled_y + 1)) + scaled_x;
    [row1, row1 + 1, row2, row2 + 1]
}

struct LightCycleShow {
    cycles: Vec<LightCycle>,
    last_frame: usize,
}

impl Show for LightCycleShow {
    fn frame(mut self, frame: usize, picture: &mut Picture) -> Self {
        if frame == 0 {
            set_constant(0, picture.as_mut_slice(0).unwrap());
            set_constant(128, picture.as_mut_slice(1).unwrap());
            set_constant(128, picture.as_mut_slice(2).unwrap());
        }

        let dt = match frame.checked_sub(self.last_frame) {
            Some(dt) => dt as f32,
            None => return self,
        };
        self.last_frame = frame;

        for cycle in &mut self.cycles {
            let new_x = cycle.x + dt * cycle.dx;
            let new_y = cycle.y + dt * cycle.dy;

            // TODO this is the WRONG WAY TO HANDLE hitting the edge.
            // Cycles shouldn't stop and make a decision, they should
            // keep moving.
            if new_x < 0f32 || new_x >= UV_WIDTH as f32 {
                cycle.dy = cycle.dx;
                cycle.dx = 0f32;
                eprintln!(
                    "TURNING, cycle at {} {} / {} {}",
                    cycle.x, cycle.y, cycle.dx, cycle.dy
                );
            } else if new_y < 0f32 || new_y >= UV_HEIGHT as f32 {
                cycle.dx = -cycle.dy;
                cycle.dy = 0f32;
            } else {
                let plot = |x: usize, y: usize, intensity: f32| {
                    if intensity == 0.0 {
                        return;
                    }

                    if !(0..UV_WIDTH).contains(&x) || !(0..UV_HEIGHT).contains(&y) {
                        return;
                    }
                    let luma = (intensity * cycle.color.y as f32) as u8;
                    let y_plane = picture.as_mut_slice(0).unwrap();
                    for ix in y_indexes(x, y) {
                        y_plane[ix] = luma;
                    }

                    let u_plane = picture.as_mut_slice(1).unwrap();
                    u_plane[uv_index(x, y)] = cycle.color.u;

                    let v_plane = picture.as_mut_slice(2).unwrap();
                    v_plane[uv_index(x, y)] = cycle.color.v;
                };

                line::rasterize_line((cycle.x, cycle.y), (new_x, new_y), plot);
                cycle.x = new_x;
                cycle.y = new_y;
            }
        }

        self
    }
}

fn main() {
    let show = LightCycleShow {
        last_frame: 0,
        cycles: vec![LightCycle {
            color: Yuv {
                y: 255,
                u: 255,
                v: 0,
            },
            x: (UV_WIDTH / 2) as f32,
            y: (UV_HEIGHT / 2) as f32,
            dx: 1.0,
            dy: 0.0,
        }],
    };
    simple::stream(show, Some(1000));
}

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}
