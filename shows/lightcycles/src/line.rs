// Rasterization algorithm from
// https://en.wikipedia.org/wiki/Xiaolin_Wu%27s_line_algorithm
// We draw "pairs of pixels straddling the line" for all lines.
//
// the given plot will not be called in any particular order.
pub fn rasterize_line<F, T>((x1, y1): (f32, f32), (x2, y2): (f32, f32), plot: F) -> Result<(), T>
where
    F: FnMut(isize, isize, f32) -> Result<(), T>,
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
    if dx == 0.0 && dy == 0.0 {
        // Special case, just color in the point
        return use_plot(ux1 as isize, uy1 as isize, 1.0);
    }
    let slope = if dx == 0.0 { 1.0 } else { dy / dx };

    let xstart = ux1.floor();
    let xstart_pixel = xstart as isize;
    let xend_pixel = ux2.ceil() as isize;

    let mut y = uy1 + slope * (xstart - ux1);
    for x_pixel in xstart_pixel..=xend_pixel {
        y += slope;
        let y_fract = y.fract();
        let y_pixel = y as isize;
        use_plot(x_pixel, y_pixel, 1.0 - y_fract)?;
        use_plot(x_pixel, y_pixel + 1, y_fract)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point() {
        let mut plots = Vec::new();
        let _ = rasterize_line((0.0, 0.0), (0.0, 0.0), |x, y, i| -> Result<(), ()> {
            plots.push((x, y, i));
            Ok(())
        });
        assert_eq!(vec![(0, 0, 1.0)], plots);
    }

    #[test]
    fn test_horizontal_positive() {
        let mut plots = Vec::new();
        let _ = rasterize_line((0.0, 0.0), (1.0, 0.0), |x, y, i| -> Result<(), ()> {
            plots.push((x, y, i));
            Ok(())
        });
        assert_eq!(
            vec![(0, 0, 1.0), (0, 1, 0.0), (1, 0, 1.0), (1, 1, 0.0)],
            plots
        );
    }

    #[test]
    fn test_horizontal_long_positive() {
        let mut plots = Vec::new();
        let _ = rasterize_line((0.0, 0.0), (4.0, 0.0), |x, y, i| -> Result<(), ()> {
            plots.push((x, y, i));
            Ok(())
        });
        assert_eq!(
            vec![
                (0, 0, 1.0),
                (0, 1, 0.0),
                (1, 0, 1.0),
                (1, 1, 0.0),
                (2, 0, 1.0),
                (2, 1, 0.0),
                (3, 0, 1.0),
                (3, 1, 0.0),
                (4, 0, 1.0),
                (4, 1, 0.0)
            ],
            plots
        )
    }

    #[test]
    fn test_vertical_positive() {
        let mut plots = Vec::new();
        let _ = rasterize_line((0.0, 0.0), (0.0, 1.0), |x, y, i| -> Result<(), ()> {
            plots.push((x, y, i));
            Ok(())
        });
        assert_eq!(
            vec![(0, 0, 1.0), (1, 0, 0.0), (0, 1, 1.0), (1, 1, 0.0)],
            plots
        )
    }
}
