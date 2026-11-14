// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "DitchAutomation",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "DitchAutomation", type: .dynamic, targets: ["DitchAutomation"]),
        // Explicit feasibility executable; never copied into the application bundle.
        .executable(name: "DitchAutomationProbe", targets: ["DitchAutomationProbe"]),
    ],
    dependencies: [.package(path: "../../../../vendor/peekaboo")],
    targets: [
        .target(name: "DitchAutomation", dependencies: [
            .product(name: "PeekabooFoundation", package: "peekaboo"),
            .product(name: "PeekabooAutomationKit", package: "peekaboo"),
            .product(name: "PeekabooBridge", package: "peekaboo"),
        ]),
        .executableTarget(name: "DitchAutomationProbe", dependencies: ["DitchAutomation"]),
        .testTarget(name: "DitchAutomationTests", dependencies: ["DitchAutomation"]),
    ],
    swiftLanguageModes: [.v6]
)
