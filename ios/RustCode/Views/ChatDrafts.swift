import Foundation
import Observation
import RustCodeRemote

/// Lives above navigation and transcript loading, so a reconnect or sidebar selection cannot erase a draft.
@MainActor
@Observable
final class ChatDrafts {
    struct Draft {
        var text = ""
        var images: [PromptImage] = []
    }

    private(set) var values: [String: Draft] = [:]

    func draft(for sessionID: String) -> Draft { values[sessionID] ?? Draft() }

    func setText(_ text: String, for sessionID: String) {
        var value = draft(for: sessionID)
        value.text = text
        values[sessionID] = value
    }

    func setImages(_ images: [PromptImage], for sessionID: String) {
        var value = draft(for: sessionID)
        value.images = images
        values[sessionID] = value
    }

    func clearIfMatching(text: String, images: [PromptImage], sessionID: String) {
        let value = draft(for: sessionID)
        guard value.text.trimmingCharacters(in: .whitespacesAndNewlines) == text, value.images == images else { return }
        values[sessionID] = Draft()
    }
}
