// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "GrokComputerUse",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .library(name: "ComputerUseCore", targets: ["ComputerUseCore"]),
        .executable(name: "GrokComputerUseApp", targets: ["GrokComputerUseApp"]),
        .executable(name: "grok-computer-use-mcp", targets: ["GrokComputerUseMCP"]),
    ],
    targets: [
        .target(
            name: "ComputerUseCore"
        ),
        .executableTarget(
            name: "GrokComputerUseApp",
            dependencies: ["ComputerUseCore"],
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("ImageIO"),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("Security"),
                .linkedFramework("UniformTypeIdentifiers"),
                .linkedLibrary("sqlite3"),
            ]
        ),
        .executableTarget(
            name: "GrokComputerUseMCP",
            dependencies: ["ComputerUseCore"],
            linkerSettings: [
                .linkedFramework("Security"),
            ]
        ),
        .testTarget(
            name: "ComputerUseCoreTests",
            dependencies: ["ComputerUseCore"]
        ),
    ]
)
