// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Said",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "Said", targets: ["Said"]),
    ],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.9.1"),
    ],
    targets: [
        .executableTarget(
            name: "Said",
            dependencies: [
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Said",
            resources: [
                .process("Resources"),
            ],
            swiftSettings: [
                .swiftLanguageMode(.v5),
            ]
        ),
    ]
)
