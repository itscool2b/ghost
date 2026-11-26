import GhostMarks
import GhostWire
import SwiftUI

/// What a chat's browser tab or emulator shows now, refreshed every few seconds while open.
struct ScreenView: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let computer: ComputerStore
    let chatId: String
    @State private var kind: ScreenKind = .browser
    @State private var image: UIImage?
    @State private var taken: Date?
    @State private var failure: String?
    @State private var live = true
    @State private var zoom: CGFloat = 1
    @State private var settledZoom: CGFloat = 1

    var body: some View {
        NavigationStack {
            VStack(spacing: 12) {
                Picker("Screen", selection: $kind) {
                    Text("Browser").tag(ScreenKind.browser)
                    Text("Device").tag(ScreenKind.device)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                ZStack {
                    if let image {
                        GeometryReader { geometry in
                            // The whole picture fits at first; pinching or a double tap looks closer.
                            let fit = fitted(
                                image.size,
                                in: CGSize(width: geometry.size.width - 32, height: geometry.size.height - 8))
                            ScrollView([.horizontal, .vertical], showsIndicators: false) {
                                Image(uiImage: image)
                                    .resizable()
                                    .frame(width: fit.width * zoom, height: fit.height * zoom)
                                    .clipShape(.rect(cornerRadius: 12))
                                    .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(style.line))
                                    .padding(.horizontal, 16)
                                    .frame(minWidth: geometry.size.width, minHeight: geometry.size.height)
                                    .accessibilityLabel(kind == .browser ? "The chat's browser tab" : "The chat's device")
                                    .accessibilityAddTraits(.isImage)
                            }
                            .scrollBounceBehavior(.basedOnSize, axes: [.horizontal, .vertical])
                            .gesture(
                                MagnifyGesture()
                                    .onChanged { zoom = min(4, max(1, settledZoom * $0.magnification)) }
                                    .onEnded { _ in settledZoom = zoom }
                            )
                            .onTapGesture(count: 2) {
                                withAnimation(.snappy) { zoom = zoom > 1 ? 1 : 2 }
                                settledZoom = zoom
                            }
                        }
                    } else if let failure {
                        ContentUnavailableView(failure, systemImage: kind == .browser ? "safari" : "iphone")
                    } else {
                        PixelLoader(size: 18, tempo: .thinking)
                    }
                }
                .frame(maxHeight: .infinity)
                HStack {
                    if let taken {
                        TimelineView(.periodic(from: .now, by: 1)) { context in
                            Text("Taken \(Int(context.date.timeIntervalSince(taken)))s ago")
                                .monospacedDigit()
                        }
                        .ghostFont(.small)
                        .foregroundStyle(style.fg3)
                    }
                    Spacer()
                    Toggle("Keep it fresh", isOn: $live).toggleStyle(.switch).fixedSize().ghostFont(.small)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            }
            .background(style.pane.ignoresSafeArea())
            .navigationTitle("Screen")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
                if let image {
                    ToolbarItem(placement: .primaryAction) {
                        ShareLink(item: Image(uiImage: image), preview: SharePreview("Screen", image: Image(uiImage: image)))
                    }
                }
            }
            .task(id: kind) {
                image = nil
                failure = nil
                zoom = 1
                settledZoom = 1
                while !Task.isCancelled {
                    await take()
                    try? await Task.sleep(for: .seconds(3))
                    while !live, !Task.isCancelled { try? await Task.sleep(for: .milliseconds(300)) }
                }
            }
        }
    }

    /// The largest size with the picture's shape that fits in `box`.
    private func fitted(_ size: CGSize, in box: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0, box.width > 0, box.height > 0 else { return .zero }
        let scale = min(box.width / size.width, box.height / size.height)
        return CGSize(width: size.width * scale, height: size.height * scale)
    }

    private func take() async {
        do {
            let picture = try await computer.connection.send(.picture(chatId, of: kind), as: ScreenPicture.self)
            if let data = Data(base64Encoded: picture.data), let shown = UIImage(data: data) {
                image = shown
                taken = .now
                failure = nil
            }
        } catch {
            if image == nil { failure = (error as? LocalizedError)?.errorDescription ?? "There is no screen to show." }
        }
    }
}
