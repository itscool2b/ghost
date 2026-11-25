// swift-tools-version: 6.2
// O7Kit: everything in the iPhone app that is not a screen. Each module builds and tests on a
// Mac (`swift test --package-path apps/ios/O7Kit`), so the protocol, the transcript and the theme
// are checked without a simulator. It depends on nothing outside Apple's SDKs.

import PackageDescription

let strict: [SwiftSetting] = [
    .enableUpcomingFeature("ExistentialAny"),
    .enableUpcomingFeature("InternalImportsByDefault"),
]

let package = Package(
    name: "O7Kit",
    defaultLocalization: "en",
    platforms: [.iOS(.v26), .macOS(.v26)],
    products: [
        .library(name: "O7Wire", targets: ["O7Wire"]),
        .library(name: "O7Crypto", targets: ["O7Crypto"]),
        .library(name: "O7Client", targets: ["O7Client"]),
        .library(name: "O7Transcript", targets: ["O7Transcript"]),
        .library(name: "O7Store", targets: ["O7Store"]),
        .library(name: "O7Theme", targets: ["O7Theme"]),
        .library(name: "O7Marks", targets: ["O7Marks"]),
        .library(name: "O7Markdown", targets: ["O7Markdown"]),
        .library(name: "O7Highlight", targets: ["O7Highlight"]),
        .library(name: "O7MockHost", targets: ["O7MockHost"]),
        .library(name: "O7Push", targets: ["O7Push"]),
    ],
    targets: [
        // The remote protocol's messages, as the desktop's `bindings.ts` and `wire.rs` give them.
        .target(name: "O7Wire", swiftSettings: strict),
        // The sealed channel (`channel.ts`, `crypto.rs`), pairing links and push seals.
        .target(name: "O7Crypto", swiftSettings: strict),
        // One computer's session through the relay (`client.ts`).
        .target(
            name: "O7Client",
            dependencies: ["O7Wire", "O7Crypto"],
            swiftSettings: strict
        ),
        // The desktop's transcript reducer (`transcript.ts` and its helpers), ported line by line.
        .target(name: "O7Transcript", dependencies: ["O7Wire"], swiftSettings: strict),
        // Paired computers, their keys in the Keychain, and what the widgets read.
        .target(
            name: "O7Store", dependencies: ["O7Wire", "O7Crypto", "O7Client"], swiftSettings: strict),
        // The desktop's ten themes, generated from `packages/ui/src/graphite/themes.ts`.
        .target(name: "O7Theme", swiftSettings: strict),
        // o7's own marks: the agent tile, the pixel loader, the agents' logos and the fonts.
        .target(
            name: "O7Marks",
            dependencies: ["O7Theme"],
            resources: [.copy("Fonts")],
            swiftSettings: strict
        ),
        // Markdown as blocks the chat draws, from Foundation's own parser.
        .target(name: "O7Markdown", swiftSettings: strict),
        // Code colours from the desktop's highlight.js, run in JavaScriptCore. The bundle copies
        // the file itself: a folder named Resources at a bundle's top fails codesign on iOS.
        .target(
            name: "O7Highlight",
            dependencies: ["O7Theme"],
            resources: [.copy("Resources/highlight.js")],
            swiftSettings: strict
        ),
        // A computer in a box: the host side of the sealed protocol with a scripted agent, for
        // tests, previews and the app's mock mode.
        .target(
            name: "O7MockHost",
            dependencies: ["O7Wire", "O7Crypto", "O7Client"],
            swiftSettings: strict
        ),
        // Sealed notifications opened on the phone, their words, the keys the notification
        // extension shares with the app, and what the widgets and the Live Activity read. Light
        // enough for an extension: no network, no SwiftUI views, no JavaScript.
        .target(name: "O7Push", dependencies: ["O7Wire", "O7Crypto"], swiftSettings: strict),
        .testTarget(name: "O7PushTests", dependencies: ["O7Push", "O7Wire", "O7Crypto"]),
        .testTarget(
            name: "O7WireTests",
            dependencies: ["O7Wire"],
            resources: [.copy("Fixtures")]
        ),
        .testTarget(name: "O7CryptoTests", dependencies: ["O7Crypto"]),
        .testTarget(name: "O7StoreTests", dependencies: ["O7Store", "O7Client"]),
        .testTarget(
            name: "O7ClientTests",
            dependencies: ["O7Client", "O7MockHost", "O7Wire", "O7Crypto"]
        ),
        .testTarget(
            name: "O7TranscriptTests",
            dependencies: ["O7Transcript", "O7Wire"],
            resources: [.copy("Goldens")]
        ),
        .testTarget(name: "O7ThemeTests", dependencies: ["O7Theme"]),
        .testTarget(name: "O7MarksTests", dependencies: ["O7Marks", "O7Theme"]),
        .testTarget(name: "O7MarkdownTests", dependencies: ["O7Markdown"]),
        .testTarget(name: "O7HighlightTests", dependencies: ["O7Highlight", "O7Theme"]),
    ]
)
