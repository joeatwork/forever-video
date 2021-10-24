use stream::Show;

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

const UV_WIDTH: usize = stream::WIDTH >> 1;
const UV_HEIGHT: usize = stream::HEIGHT >> 1;
const CYCLE_SENSE_RANGE: f32 = 8.0;

#[inline]
fn uv_index(x: usize, y: usize) -> usize {
    (UV_WIDTH * y) + x
}

#[inline]
fn y_indexes(x: usize, y: usize) -> [usize; 4] {
    let scaled_x = x * 2;
    let scaled_y = y * 2;
    let row1 = (stream::WIDTH * scaled_y) + scaled_x;
    let row2 = (stream::WIDTH * (scaled_y + 1)) + scaled_x;
    [row1, row1 + 1, row2, row2 + 1]
}

struct LightCycleShow {
    cycles: Vec<LightCycle>,
    last_frame: usize,
}

impl Show for LightCycleShow {
    fn frame(
        mut self,
        frame: usize,
        y_plane: &mut [u8],
        u_plane: &mut [u8],
        v_plane: &mut [u8],
    ) -> Self {
        if frame == 0 {
            set_constant(0, y_plane);
            set_constant(128, u_plane);
            set_constant(128, v_plane);
        }

        let dt = match frame.checked_sub(self.last_frame) {
            Some(0) | None => return self,
            Some(dt) => dt as f32,
        };
        self.last_frame = frame;

        let mut alive = false;
        for cycle in &mut self.cycles {
            let try_dx = cycle.dx * dt;
            let try_dy = cycle.dy * dt;

            let has_clear_path = if path_is_clear(cycle, try_dx, try_dy, y_plane) {
                true
            } else if path_is_clear(cycle, -try_dy, try_dx, y_plane) {
                let new_dx = -cycle.dy;
                cycle.dy = cycle.dx;
                cycle.dx = new_dx;
                true
            } else if path_is_clear(cycle, try_dy, -try_dx, y_plane) {
                let new_dy = -cycle.dx;
                cycle.dx = cycle.dy;
                cycle.dy = new_dy;
                true
            } else {
                false
            };

            if has_clear_path {
                alive = true;
                let move_dx = cycle.dx * dt;
                let move_dy = cycle.dy * dt;
                let new_x = cycle.x + move_dx;
                let new_y = cycle.y + move_dy;

                let _ = line::rasterize_line(
                    (cycle.x, cycle.y),
                    (new_x, new_y),
                    |ix, iy, intensity| -> Result<(), ()> {
                        assert!(ix >= 0);
                        assert!(iy >= 0);
                        let x = ix as usize;
                        let y = iy as usize;

                        let luma = (intensity * cycle.color.y as f32) as u8;
                        for ix in y_indexes(x as usize, y as usize) {
                            y_plane[ix] = luma;
                        }

                        u_plane[uv_index(x, y)] = cycle.color.u;
                        v_plane[uv_index(x, y)] = cycle.color.v;

                        Ok(())
                    },
                );

                cycle.x = new_x;
                cycle.y = new_y;
            }
        }

        if !alive {
            set_constant(0, y_plane);
            set_constant(128, u_plane);
            set_constant(128, v_plane);
        }

        self
    }
}

fn path_is_clear(cycle: &LightCycle, move_dx: f32, move_dy: f32, y_plane: &mut [u8]) -> bool {
    let hyp_squared = move_dx * move_dx + move_dy * move_dy;
    let (sense_range_x, sense_range_y) = if CYCLE_SENSE_RANGE * CYCLE_SENSE_RANGE > hyp_squared {
        let hyp = hyp_squared.sqrt();
        let norm_dx = move_dx / hyp;
        let norm_dy = move_dy / hyp;

        (norm_dx * CYCLE_SENSE_RANGE, norm_dy * CYCLE_SENSE_RANGE)
    } else {
        (move_dx, move_dy)
    };

    let vision_x = cycle.x + sense_range_x;
    let vision_y = cycle.y + sense_range_y;

    let overdrew = line::rasterize_line(
        (cycle.x, cycle.y),
        (vision_x, vision_y),
        |x, y, intensity| {
            if intensity == 0.0 {
                return Ok(());
            }

            if !(0..UV_WIDTH as isize).contains(&x) || !(0..UV_HEIGHT as isize).contains(&y) {
                return Err((x, y));
            }

            if (cycle.x - 1.0..=cycle.x + 1.0).contains(&(x as f32))
                && (cycle.y - 1.0..=cycle.y + 1.0).contains(&(y as f32))
            {
                return Ok(());
            }

            for ix in y_indexes(x as usize, y as usize) {
                if y_plane[ix] != 0 {
                    return Err((x, y));
                }
            }

            Ok(())
        },
    );

    overdrew.is_ok()
}

fn main() {
    let show = LightCycleShow {
        last_frame: 0,
        cycles: vec![
            LightCycle {
                color: Yuv {
                    y: 255,
                    u: 255,
                    v: 0,
                },
                x: (UV_WIDTH / 2) as f32,
                y: (UV_HEIGHT / 2) as f32,
                dx: 0.0,
                dy: 6.0,
            },
            LightCycle {
                color: Yuv {
                    y: 255,
                    u: 0,
                    v: 255,
                },
                x: (UV_WIDTH / 2) as f32,
                y: (UV_HEIGHT / 2) as f32,
                dx: -5.0,
                dy: 0.0,
            },
            LightCycle {
                color: Yuv {
                    y: 255,
                    u: 20,
                    v: 150,
                },
                x: (UV_WIDTH / 2) as f32,
                y: (UV_HEIGHT / 2) as f32,
                dx: -5.0,
                dy: 0.0,
            },
        ],
    };
    stream::stream(show, None, None);
}

fn set_constant(val: u8, buf: &mut [u8]) {
    for x in buf {
        *x = val
    }
}
