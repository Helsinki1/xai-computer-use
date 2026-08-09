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

    /// Projects a global accessibility frame into the screenshot that produced
    /// `geometry`. The intersection is deliberate: accessibility elements may
    /// extend beyond the captured window, while the returned rectangle must
    /// describe only visible screenshot pixels.
    public static func pngRect(
        for globalRect: GlobalScreenRect,
        in geometry: WindowGeometry
    ) -> PNGPixelRect? {
        let bounds = geometry.globalBoundsPoints
        guard geometry.pngWidthPixels > 0,
              geometry.pngHeightPixels > 0,
              bounds.x.isFinite,
              bounds.y.isFinite,
              bounds.width.isFinite,
              bounds.height.isFinite,
              bounds.width > 0,
              bounds.height > 0,
              globalRect.x.isFinite,
              globalRect.y.isFinite,
              globalRect.width.isFinite,
              globalRect.height.isFinite,
              globalRect.width > 0,
              globalRect.height > 0
        else {
            return nil
        }

        let left = max(globalRect.x, bounds.x)
        let top = max(globalRect.y, bounds.y)
        let right = min(globalRect.x + globalRect.width, bounds.x + bounds.width)
        let bottom = min(globalRect.y + globalRect.height, bounds.y + bounds.height)
        guard right > left, bottom > top else { return nil }

        let scaleX = Double(geometry.pngWidthPixels) / bounds.width
        let scaleY = Double(geometry.pngHeightPixels) / bounds.height
        return PNGPixelRect(
            x: (left - bounds.x) * scaleX,
            y: (top - bounds.y) * scaleY,
            width: (right - left) * scaleX,
            height: (bottom - top) * scaleY
        )
    }
}
