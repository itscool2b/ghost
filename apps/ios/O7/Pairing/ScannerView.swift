import O7Marks
import SwiftUI
import VisionKit

/// The camera, looking for the pairing code o7 shows on the computer. A code that is not o7's is
/// said so; one that is goes to the question before anything connects.
struct ScannerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    @State private var found = 0

    private var available: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    var body: some View {
        ZStack {
            if available {
                CodeScanner { text in
                    found += 1
                    model.propose(text)
                }
                .ignoresSafeArea()
                Viewfinder()
            } else {
                Color.black.ignoresSafeArea()
                VStack(spacing: 14) {
                    Image(systemName: "camera.metering.unknown")
                        .font(.largeTitle.weight(.light))
                        .foregroundStyle(.white.opacity(0.7))
                    Text("The camera is not available")
                        .o7Font(.heading)
                        .foregroundStyle(.white)
                    Text("Allow o7 to use the camera in Settings, or paste the pairing link o7 shows under the code.")
                        .o7Font(.prose)
                        .foregroundStyle(.white.opacity(0.7))
                        .multilineTextAlignment(.center)
                    Button("Open Settings") {
                        if let url = URL(string: UIApplication.openSettingsURLString) {
                            UIApplication.shared.open(url)
                        }
                    }
                    .buttonStyle(.glass)
                }
                .padding(32)
            }
            VStack {
                HStack {
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "xmark").font(.body.weight(.semibold))
                            .frame(width: 30, height: 30)
                    }
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .accessibilityLabel("Close")
                    Spacer()
                }
                Spacer()
                VStack(spacing: 14) {
                    Text("Point at the code in o7's Settings on your computer")
                        .o7Font(.ui(15, weight: .medium))
                        .foregroundStyle(.white)
                        .multilineTextAlignment(.center)
                        .shadow(color: .black.opacity(0.4), radius: 6)
                    PasteLinkButton().frame(maxWidth: 280)
                }
            }
            .padding(20)
        }
        .sensoryFeedback(.success, trigger: found)
        .preferredColorScheme(.dark)
    }
}

/// A rounded window in a dark veil, with corners, where the code should be.
private struct Viewfinder: View {
    var body: some View {
        GeometryReader { space in
            let side = min(space.size.width, space.size.height) * 0.68
            let frame = CGRect(
                x: (space.size.width - side) / 2, y: (space.size.height - side) / 2 - 30,
                width: side, height: side)
            ZStack {
                Rectangle()
                    .fill(.black.opacity(0.55))
                    .mask {
                        Rectangle()
                            .overlay {
                                RoundedRectangle(cornerRadius: 28)
                                    .frame(width: frame.width, height: frame.height)
                                    .position(x: frame.midX, y: frame.midY)
                                    .blendMode(.destinationOut)
                            }
                            .compositingGroup()
                    }
                RoundedRectangle(cornerRadius: 28)
                    .strokeBorder(.white.opacity(0.9), style: StrokeStyle(lineWidth: 3, lineCap: .round, dash: [34, side - 34]))
                    .frame(width: frame.width, height: frame.height)
                    .position(x: frame.midX, y: frame.midY)
            }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
    }
}

/// VisionKit's live QR scanner.
private struct CodeScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced,
            recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false,
            isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        controller.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onCode: (String) -> Void
        private var last: String?

        init(onCode: @escaping (String) -> Void) {
            self.onCode = onCode
        }

        func dataScanner(
            _ scanner: DataScannerViewController, didAdd added: [RecognizedItem], allItems: [RecognizedItem]
        ) {
            for item in added {
                guard case .barcode(let code) = item, let text = code.payloadStringValue, text != last else { continue }
                last = text
                onCode(text)
                return
            }
        }
    }
}
