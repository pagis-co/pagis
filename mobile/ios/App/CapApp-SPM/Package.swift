// swift-tools-version: 5.9
import PackageDescription

// DO NOT MODIFY THIS FILE - managed by Capacitor CLI commands
let package = Package(
    name: "CapApp-SPM",
    platforms: [.iOS(.v16)],
    products: [
        .library(
            name: "CapApp-SPM",
            targets: ["CapApp-SPM"])
    ],
    dependencies: [
        .package(url: "https://github.com/ionic-team/capacitor-swift-pm.git", exact: "8.5.2"),
        .package(name: "CapacitorApp", path: "../../../../node_modules/.pnpm/@capacitor+app@8.1.2_@capacitor+core@8.5.2/node_modules/@capacitor/app"),
        .package(name: "CapacitorBarcodeScanner", path: "../../../../node_modules/.pnpm/@capacitor+barcode-scanner@3.1.2_@capacitor+core@8.5.2/node_modules/@capacitor/barcode-scanner"),
        .package(name: "CapacitorBrowser", path: "../../../../node_modules/.pnpm/@capacitor+browser@8.0.5_@capacitor+core@8.5.2/node_modules/@capacitor/browser"),
        .package(name: "CapacitorFileViewer", path: "../../../../node_modules/.pnpm/@capacitor+file-viewer@2.0.4_@capacitor+core@8.5.2/node_modules/@capacitor/file-viewer"),
        .package(name: "CapacitorFilesystem", path: "../../../../node_modules/.pnpm/@capacitor+filesystem@8.1.5_@capacitor+core@8.5.2/node_modules/@capacitor/filesystem")
    ],
    targets: [
        .target(
            name: "CapApp-SPM",
            dependencies: [
                .product(name: "Capacitor", package: "capacitor-swift-pm"),
                .product(name: "Cordova", package: "capacitor-swift-pm"),
                .product(name: "CapacitorApp", package: "CapacitorApp"),
                .product(name: "CapacitorBarcodeScanner", package: "CapacitorBarcodeScanner"),
                .product(name: "CapacitorBrowser", package: "CapacitorBrowser"),
                .product(name: "CapacitorFileViewer", package: "CapacitorFileViewer"),
                .product(name: "CapacitorFilesystem", package: "CapacitorFilesystem")
            ]
        )
    ]
)
