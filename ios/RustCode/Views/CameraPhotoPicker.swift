import SwiftUI
import UIKit

/// The system camera supplies a photo to the same import pipeline used by Photos and Files.
struct CameraPhotoPicker: UIViewControllerRepresentable {
    let completion: (Result<Data?, Error>) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(completion: completion) }

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.mediaTypes = ["public.image"]
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: UIImagePickerController, context: Context) {}

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let completion: (Result<Data?, Error>) -> Void
        init(completion: @escaping (Result<Data?, Error>) -> Void) { self.completion = completion }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            let image = info[.originalImage] as? UIImage
            guard let data = image?.jpegData(compressionQuality: 0.9) else {
                completion(.failure(PromptImageImport.ImportError.unreadable))
                return
            }
            completion(.success(data))
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) { completion(.success(nil)) }
    }
}
