import PhotosUI
import SwiftUI
import UIKit

/// A photo the person picked or took, made ready to send: a JPEG at most 1,600 points along its
/// longer side, as the computer takes images.
struct PickedPhoto: Identifiable, Equatable {
    let id = UUID()
    let image: UIImage
    let jpeg: Data

    init?(image: UIImage) {
        let longest = max(image.size.width, image.size.height)
        let scale = longest > 1600 ? 1600 / longest : 1
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        guard let jpeg = resized.jpegData(compressionQuality: 0.8) else { return nil }
        self.image = resized
        self.jpeg = jpeg
    }

    static func == (lhs: PickedPhoto, rhs: PickedPhoto) -> Bool { lhs.id == rhs.id }

    /// The most photos one message takes, together, as the computer counts them.
    static let budget = 8 * 1024 * 1024

    /// Why these photos cannot go together, or nil when they can. The computer refuses the same,
    /// before it makes anything; saying it here spares the upload.
    static func overBudget(_ photos: [PickedPhoto]) -> String? {
        photos.reduce(0) { $0 + $1.jpeg.count } > budget
            ? "These photos are more than 8 MB together. Send fewer at a time." : nil
    }
}

/// The photos waiting to go with a message, each with a way to take it back out.
struct PhotoStrip: View {
    @Environment(\.style) private var style
    @Binding var photos: [PickedPhoto]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(photos) { photo in
                    Image(uiImage: photo.image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: 64, height: 64)
                        .clipShape(.rect(cornerRadius: 12))
                        .overlay(alignment: .topTrailing) {
                            Button {
                                withAnimation(.snappy) { photos.removeAll { $0 == photo } }
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .symbolRenderingMode(.palette)
                                    .foregroundStyle(.white, .black.opacity(0.6))
                                    .font(.body)
                                    .frame(width: 44, height: 44, alignment: .topTrailing)
                                    .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Remove the photo")
                            .offset(x: 6, y: -6)
                        }
                }
            }
            .padding(.horizontal, 12)
            .padding(.top, 6)
        }
    }
}

/// "+": a photo from the library, or one taken now. The pickers themselves hang off the screen
/// (`photoSources`): presented from inside the floating composer, they never appear.
struct PhotoButton: View {
    @Environment(\.style) private var style
    @Binding var picking: Bool
    @Binding var shooting: Bool

    var body: some View {
        // Plain inside the composer's glass: a glass menu there never takes a tap on its items.
        Menu {
            Button("Photo library", systemImage: "photo.on.rectangle") { picking = true }
            if UIImagePickerController.isSourceTypeAvailable(.camera) {
                Button("Take a photo", systemImage: "camera") { shooting = true }
            }
        } label: {
            Image(systemName: "plus")
                .font(.title3.weight(.medium))
                .foregroundStyle(style.fg2)
                .frame(width: 44, height: 44)
                .contentShape(.rect)
        }
        .accessibilityLabel("Add a photo")
    }
}

/// At most this many photos go with one message.
let photoLimit = 8

extension View {
    /// The photo library and the camera, adding what the person picks to `photos`.
    func photoSources(_ photos: Binding<[PickedPhoto]>, picking: Binding<Bool>, shooting: Binding<Bool>) -> some View {
        modifier(PhotoSources(photos: photos, picking: picking, shooting: shooting))
    }
}

private struct PhotoSources: ViewModifier {
    @Binding var photos: [PickedPhoto]
    @Binding var picking: Bool
    @Binding var shooting: Bool
    @State private var selection: [PhotosPickerItem] = []

    func body(content: Content) -> some View {
        content
            .photosPicker(
                isPresented: $picking, selection: $selection,
                maxSelectionCount: max(1, photoLimit - photos.count), matching: .images)
            .onChange(of: selection) { _, items in
                guard !items.isEmpty else { return }
                Task {
                    for item in items where photos.count < photoLimit {
                        if let data = try? await item.loadTransferable(type: Data.self),
                            let image = UIImage(data: data), let photo = PickedPhoto(image: image)
                        {
                            photos.append(photo)
                        }
                    }
                    selection = []
                }
            }
            .fullScreenCover(isPresented: $shooting) {
                Camera { image in
                    if let image, photos.count < photoLimit, let photo = PickedPhoto(image: image) {
                        photos.append(photo)
                    }
                    shooting = false
                }
                .ignoresSafeArea()
            }
    }
}

/// The system camera, for a photo to send.
private struct Camera: UIViewControllerRepresentable {
    let done: (UIImage?) -> Void

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(done: done) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let done: (UIImage?) -> Void
        init(done: @escaping (UIImage?) -> Void) { self.done = done }

        func imagePickerController(
            _ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
        ) {
            done(info[.originalImage] as? UIImage)
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) { done(nil) }
    }
}

extension ComputerStore {
    /// Sends the photos to the computer, one after another, and gives their ids for a message.
    func upload(_ photos: [PickedPhoto]) async throws -> [String] {
        var ids: [String] = []
        for (index, photo) in photos.enumerated() {
            ids.append(try await connection.upload(photo.jpeg, name: "photo-\(index + 1).jpg", mime: "image/jpeg"))
        }
        return ids
    }
}
