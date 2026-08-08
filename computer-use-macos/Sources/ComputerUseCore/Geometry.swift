import Foundation

public enum CoordinateMapper {
    /// Maps continuous PNG pixel-edge coordinates into Quartz global point space.
    /// There is deliberately no half-pixel offset, rounding, clamping, Y flip, or
    /// backing-scale inference in this conversion.
    public static func globalPoint(
        for pixel: PNGPixelPoint,
        in geometry: WindowGeometry
    ) throws -> GlobalScreenPoint {
        let bounds = geometry.globalBoundsPoints
        guard geometry.pngWidthPixels > 0,
              geometry.pngHeightPixels > 0,
              bounds.x.isFinite,
              bounds.y.isFinite,
              bounds.width.isFinite,
              bounds.height.isFinite,
              bounds.width > 0,
              bounds.height > 0
        else {
            throw ComputerUseError.stateUnavailable("Snapshot geometry is incomplete; a coordinate action cannot be performed.")
        }

        guard pixel.x.isFinite,
              pixel.y.isFinite,
              pixel.x >= 0,
              pixel.y >= 0,
              pixel.x < Double(geometry.pngWidthPixels),
              pixel.y < Double(geometry.pngHeightPixels)
        else {
            throw ComputerUseError.invalidArguments(
                "Pixel coordinates must satisfy 0 <= x < \(geometry.pngWidthPixels) and 0 <= y < \(geometry.pngHeightPixels)."
            )
        }

        return GlobalScreenPoint(
            x: bounds.x + pixel.x * bounds.width / Double(geometry.pngWidthPixels),
            y: bounds.y + pixel.y * bounds.height / Double(geometry.pngHeightPixels)
        )
    }
}
