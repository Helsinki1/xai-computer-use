import XCTest
@testable import ComputerUseCore

final class GeometryTests: XCTestCase {
    func testContinuousPixelEdgesMapDirectlyWithoutFlipOrHalfPixelOffset() throws {
        let geometry = WindowGeometry(
            windowIdentifier: 7,
            globalBoundsPoints: GlobalScreenRect(x: -100, y: 50, width: 400, height: 200),
            pngWidthPixels: 800,
            pngHeightPixels: 400
        )

        XCTAssertEqual(
            try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 0, y: 0), in: geometry),
            GlobalScreenPoint(x: -100, y: 50)
        )
        XCTAssertEqual(
            try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 400, y: 300), in: geometry),
            GlobalScreenPoint(x: 100, y: 200)
        )
        XCTAssertEqual(
            try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 0.5, y: 0.5), in: geometry),
            GlobalScreenPoint(x: -99.75, y: 50.25)
        )
    }

    func testHalfOpenPixelBoundsFailClosed() {
        let geometry = WindowGeometry(
            windowIdentifier: 1,
            globalBoundsPoints: GlobalScreenRect(x: 0, y: 0, width: 100, height: 100),
            pngWidthPixels: 100,
            pngHeightPixels: 100
        )
        XCTAssertThrowsError(try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: -0.01, y: 0), in: geometry))
        XCTAssertThrowsError(try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 100, y: 0), in: geometry))
        XCTAssertThrowsError(try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 0, y: 100), in: geometry))
        XCTAssertThrowsError(try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: .nan, y: 0), in: geometry))
    }

    func testMissingGeometryNeverFallsBackToIdentity() {
        let geometry = WindowGeometry(
            windowIdentifier: 1,
            globalBoundsPoints: GlobalScreenRect(x: 0, y: 0, width: 0, height: 100),
            pngWidthPixels: 100,
            pngHeightPixels: 100
        )
        XCTAssertThrowsError(try CoordinateMapper.globalPoint(for: PNGPixelPoint(x: 1, y: 1), in: geometry))
    }
}
