//! The exact pixel-to-global coordinate contract, mirroring `Geometry.swift`.

use crate::models::{ComputerUseError, GlobalScreenPoint, PngPixelPoint, Result, WindowGeometry};

/// Maps continuous PNG pixel-edge coordinates into global desktop space.
/// There is deliberately no half-pixel offset, rounding, clamping, Y flip, or
/// scale inference in this conversion.
pub fn global_point(pixel: PngPixelPoint, geometry: &WindowGeometry) -> Result<GlobalScreenPoint> {
    let bounds = &geometry.global_bounds_points;
    if geometry.png_width_pixels == 0
        || geometry.png_height_pixels == 0
        || !bounds.x.is_finite()
        || !bounds.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(ComputerUseError::StateUnavailable(
            "Snapshot geometry is incomplete; a coordinate action cannot be performed.".to_owned(),
        ));
    }

    if !pixel.x.is_finite()
        || !pixel.y.is_finite()
        || pixel.x < 0.0
        || pixel.y < 0.0
        || pixel.x >= f64::from(geometry.png_width_pixels)
        || pixel.y >= f64::from(geometry.png_height_pixels)
    {
        return Err(ComputerUseError::InvalidArguments(format!(
            "Pixel coordinates must satisfy 0 <= x < {} and 0 <= y < {}.",
            geometry.png_width_pixels, geometry.png_height_pixels
        )));
    }

    Ok(GlobalScreenPoint {
        x: bounds.x + pixel.x * bounds.width / f64::from(geometry.png_width_pixels),
        y: bounds.y + pixel.y * bounds.height / f64::from(geometry.png_height_pixels),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::GlobalScreenRect;

    fn geometry() -> WindowGeometry {
        WindowGeometry {
            window_identifier: 7,
            global_bounds_points: GlobalScreenRect {
                x: 100.0,
                y: 50.0,
                width: 400.0,
                height: 300.0,
            },
            png_width_pixels: 800,
            png_height_pixels: 600,
        }
    }

    #[test]
    fn maps_continuous_edge_coordinates_with_independent_ratios() {
        let point = global_point(PngPixelPoint { x: 0.0, y: 0.0 }, &geometry()).unwrap();
        assert_eq!(point, GlobalScreenPoint { x: 100.0, y: 50.0 });

        let point = global_point(PngPixelPoint { x: 400.0, y: 300.0 }, &geometry()).unwrap();
        assert_eq!(point, GlobalScreenPoint { x: 300.0, y: 200.0 });

        let point = global_point(
            PngPixelPoint {
                x: 799.5,
                y: 599.25,
            },
            &geometry(),
        )
        .unwrap();
        assert!((point.x - (100.0 + 799.5 * 400.0 / 800.0)).abs() < 1e-12);
        assert!((point.y - (50.0 + 599.25 * 300.0 / 600.0)).abs() < 1e-12);
    }

    #[test]
    fn rejects_out_of_range_and_non_finite_pixels() {
        for pixel in [
            PngPixelPoint { x: -0.001, y: 0.0 },
            PngPixelPoint { x: 0.0, y: -1.0 },
            PngPixelPoint { x: 800.0, y: 0.0 },
            PngPixelPoint { x: 0.0, y: 600.0 },
            PngPixelPoint {
                x: f64::NAN,
                y: 0.0,
            },
            PngPixelPoint {
                x: 0.0,
                y: f64::INFINITY,
            },
        ] {
            let error = global_point(pixel, &geometry()).unwrap_err();
            assert_eq!(error.code(), "invalid_arguments");
        }
    }

    #[test]
    fn rejects_incomplete_geometry() {
        let mut zero_width = geometry();
        zero_width.global_bounds_points.width = 0.0;
        let mut zero_png = geometry();
        zero_png.png_height_pixels = 0;
        let mut non_finite = geometry();
        non_finite.global_bounds_points.x = f64::NAN;
        for broken in [zero_width, zero_png, non_finite] {
            let error = global_point(PngPixelPoint { x: 1.0, y: 1.0 }, &broken).unwrap_err();
            assert_eq!(error.code(), "state_unavailable");
        }
    }
}
