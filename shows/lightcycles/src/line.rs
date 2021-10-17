// Rasterization algorithm from
// https://en.wikipedia.org/wiki/Xiaolin_Wu%27s_line_algorithm
// We draw "pairs of pixels straddling the line" for all lines.
pub fn rasterize_line<F>((x1, y1): (f32, f32), (x2, y2): (f32, f32), plot: F)
where
    F: FnMut(usize, usize, f32),
{
    let steep = (x1 - x2).abs() < (y1 - y2).abs();
    let mut p = plot;
    let mut use_plot = move |x, y, intensity| {
        if steep {
            p(y, x, intensity)
        } else {
            p(x, y, intensity)
        }
    };

    let ((ux1, uy1), (ux2, uy2)) = if steep {
        ((y1, x1), (y2, x2))
    } else {
        ((x1, y1), (x2, y2))
    };

    let ((ux1, uy1), (ux2, uy2)) = if ux1 > ux2 {
        ((ux2, uy2), (ux1, uy1))
    } else {
        ((ux1, uy1), (ux2, uy2))
    };

    let dx = ux2 - ux1;
    let dy = uy2 - uy1;
    let slope = if dx == 0.0 || dy == 0.0 { 1.0 } else { dy / dx };
    assert_ne!(0.0, slope); // could underflow, but it's unlikely...

    let xstart = ux1.round();
    let ystart = uy1 + slope * (xstart - ux1);

    // As far as I can tell, the 0.5 here caps the value of xdiff at 0.5
    let xstart_diff = 1.0 - (ux1 + 0.5).fract();
    let xstart_pixel = xstart as usize;
    let ystart_pixel = ystart as usize;
    let ystart_fract = ystart.fract();
    use_plot(
        xstart_pixel,
        ystart_pixel,
        (1.0 - ystart_fract) * xstart_diff,
    );
    use_plot(xstart_pixel, ystart_pixel + 1, ystart_fract * xstart_diff);

    let xend = ux2.round();
    let yend = uy2 + slope * (xend - ux2);
    let xend_diff = (ux2 + 0.5).fract();
    let xend_pixel = xend as usize;
    let yend_pixel = yend as usize;
    let yend_fract = yend.fract();
    use_plot(xend_pixel, yend_pixel, (1.0 - yend_fract) * xend_diff);
    use_plot(xend_pixel, yend_pixel + 1, yend_fract * xend_diff);

    let mut y = ystart;
    for x_pixel in xstart_pixel + 1..xend_pixel {
        y += slope;
        let y_fract = y.fract();
        let y_pixel = y as usize;
        use_plot(x_pixel, y_pixel, 1.0 - y_fract);
        use_plot(x_pixel, y_pixel + 1, y_fract);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point() {
        let mut plots: Vec<(usize, usize, f32)> = Vec::new();
        rasterize_line((0.0, 0.0), (0.0, 0.0), |x, y, i| plots.push((x, y, i)));
        // TODO this is not the behavior we really want for points in the long run
        // - if start == end we should probably just light the pixel...
        assert_eq!(
            vec![(0, 0, 0.5), (0, 1, 0.0), (0, 0, 0.5), (0, 1, 0.0)],
            plots
        );
    }

    #[test]
    fn test_horizontal_positive() {
        let mut plots: Vec<(usize, usize, f32)> = Vec::new();
        rasterize_line((0.0, 0.0), (1.0, 0.0), |x, y, i| plots.push((x, y, i)));
        assert_eq!(
            vec![(0, 0, 0.5), (0, 1, 0.0), (1, 0, 0.5), (1, 1, 0.0)],
            plots
        );
    }

    #[test]
    fn test_horizontal_long_positive() {
        let mut plots: Vec<(usize, usize, f32)> = Vec::new();
        rasterize_line((0.0, 0.0), (4.0, 0.0), |x, y, i| plots.push((x, y, i)));
        // TODO we have artifacts at the start and end of the line that we don't have
        // in the middle of the line...
        assert_eq!(
            vec![
                (0, 0, 0.5),
                (0, 1, 0.0),
                (4, 0, 0.5),
                (4, 1, 0.0),
                (1, 1, 1.0),
                (1, 2, 0.0),
                (2, 2, 1.0),
                (2, 3, 0.0),
                (3, 3, 1.0),
                (3, 4, 0.0)
            ],
            plots
        )
    }

    #[test]
    fn test_vertical_positive() {
        let mut plots: Vec<(usize, usize, f32)> = Vec::new();
        rasterize_line((0.0, 0.0), (0.0, 1.0), |x, y, i| plots.push((x, y, i)));
        assert_eq!(
            vec![(0, 0, 0.5), (1, 0, 0.0), (0, 1, 0.5), (1, 1, 0.0)],
            plots
        )
    }
}
