// swift-tools-version: 6.2
// GhostKit: everything in the iPhone app that is not a screen. Each module builds and tests on a
// Mac (`swift test --package-path apps/ios/GhostKit`), so the protocol, the transcript and the theme
// are checked without a simulator. It depends on nothing outside Apple's SDKs.

import PackageDescription

let strict: [SwiftSetting] = [
    .enableUpcomingFeature("ExistentialAny"),
    .enableUpcomingFeature("InternalImportsByDefault"),
]

let package = Package(
    name: "GhostKit",
    defaultLocalization: "en",
    platforms: [.iOS(.v26), .macOS(.v26)],
    products: [
        .library(name: "GhostWire", targets: ["GhostWire"]),
        .library(name: "GhostCrypto", targets: ["GhostCrypto"]),
        .library(name: "GhostClient", targets: ["GhostClient"]),
        .library(name: "GhostTranscript", targets: ["GhostTranscript"]),
        .library(name: "GhostStore", targets: ["GhostStore"]),
        .library(name: "GhostTheme", targets: ["GhostTheme"]),
        .library(name: "GhostMarks", targets: ["GhostMarks"]),
        .library(name: "GhostMarkdown", targets: ["GhostMarkdown"]),
        .library(name: "GhostHighlight", targets: ["GhostHighlight"]),
        .library(name: "GhostMockHost", targets: ["GhostMockHost"]),
        .library(name: "GhostPush", targets: ["GhostPush"]),
    ],
    targets: [
        // The remote protocol's messages, as the desktop's `bindings.ts` and `wire.rs` give them.
        .target(name: "GhostWire", swiftSettings: strict),
        // The sealed channel (`channel.ts`, `crypto.rs`), pairing links and push seals.
        .target(name: "GhostCrypto", swiftSettings: strict),
        // One computer's session through the relay (`client.ts`).
        .target(
            name: "GhostClient",
            dependencies: ["GhostWire", "GhostCrypto"],
            swiftSettings: strict
        ),
        // The desktop's transcript reducer (`transcript.ts` and its helpers), ported line by line.
        .target(name: "GhostTranscript", dependencies: ["GhostWire"], swiftSettings: strict),
        // Paired computers, their keys in the Keychain, and what the widgets read.
        .target(
            name: "GhostStore", dependencies: ["GhostWire", "GhostCrypto", "GhostClient"], swiftSettings: strict),
        // The desktop's ten themes, generated from `packages/ui/src/graphite/themes.ts`.
        .target(name: "GhostTheme", swiftSettings: strict),
        // Ghost's own marks: the agent tile, the pixel loader, the agents' logos and the fonts.
        .target(
            name: "GhostMarks",
            dependencies: ["GhostTheme"],
            resources: [.copy("Fonts")],
            swiftSettings: strict
        ),
        // Markdown as blocks the chat draws, from Foundation's own parser.
        .target(name: "GhostMarkdown", swiftSettings: strict),
        // Code colours from the desktop's highlight.js, run in JavaScriptCore. The bundle copies
        // the file itself: a folder named Resources at a bundle's top fails codesign on iOS.
        .target(
            name: "GhostHighlight",
            dependencies: ["GhostTheme"],
            resources: [.copy("Resources/highlight.js")],
            swiftSettings: strict
        ),
        // A computer in a box: the host side of the sealed protocol with a scripted agent, for
        // tests, previews and the app's mock mode.
        .target(
            name: "GhostMockHost",
            dependencies: ["GhostWire", "GhostCrypto", "GhostClient"],
            swiftSettings: strict
        ),
        // Sealed notifications opened on the phone, their words, the keys the notification
        // extension shares with the app, and what the widgets and the Live Activity read. Light
        // enough for an extension: no network, no SwiftUI views, no JavaScript.
        .target(name: "GhostPush", dependencies: ["GhostWire", "GhostCrypto"], swiftSettings: strict),
        .testTarget(name: "GhostPushTests", dependencies: ["GhostPush", "GhostWire", "GhostCrypto"]),
        .testTarget(
            name: "GhostWireTests",
            dependencies: ["GhostWire"],
            resources: [.copy("Fixtures")]
        ),
        .testTarget(name: "GhostCryptoTests", dependencies: ["GhostCrypto"]),
        .testTarget(name: "GhostStoreTests", dependencies: ["GhostStore", "GhostClient"]),
        .testTarget(
            name: "GhostClientTests",
            dependencies: ["GhostClient", "GhostMockHost", "GhostWire", "GhostCrypto"]
        ),
        .testTarget(
            name: "GhostTranscriptTests",
            dependencies: ["GhostTranscript", "GhostWire"],
            resources: [.copy("Goldens")]
        ),
        .testTarget(name: "GhostThemeTests", dependencies: ["GhostTheme"]),
        .testTarget(name: "GhostMarksTests", dependencies: ["GhostMarks", "GhostTheme"]),
        .testTarget(name: "GhostMarkdownTests", dependencies: ["GhostMarkdown"]),
        .testTarget(name: "GhostHighlightTests", dependencies: ["GhostHighlight", "GhostTheme"]),
    ]
)
