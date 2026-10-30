import RustCodeRemote
import SwiftUI
import PhotosUI
import UIKit
import AVFoundation
import UniformTypeIdentifiers

/// The message field and its controls, in one rounded glass composer.
struct ComposerBar<Accessory: View, Controls: View>: View {
    @Environment(RemoteStore.self) private var store
    @Environment(ChatDrafts.self) private var drafts
    let sessionID: String
    let isRunning: Bool
    /// Whether the running turn accepts steering.
    var canSteer = true
    let isEnabled: Bool
    var runCommand: (String) async -> Bool
    @ViewBuilder var accessory: () -> Accessory
    @ViewBuilder var controls: () -> Controls
    var keepsExpanded = false

    @State private var sendCount = 0
    @State private var stopCount = 0
    @State private var copiedIDCount = 0
    @Binding var showsActions: Bool
    @Binding var nextAction: ChatAction?
    @State private var showsFolder = false
    @State private var showsPhotos = false
    @State private var showsFiles = false
    @State private var showsCamera = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var isImporting = false
    @State private var isSending = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState.Binding var isFocused: Bool
    @ScaledMetric(relativeTo: .body) private var buttonSize: CGFloat = 44
    @ScaledMetric(relativeTo: .body) private var commandRowHeight: CGFloat = 58

    private var text: String {
        get { drafts.draft(for: sessionID).text }
        nonmutating set { drafts.setText(newValue, for: sessionID) }
    }

    private var images: [PromptImage] {
        get { drafts.draft(for: sessionID).images }
        nonmutating set { drafts.setImages(newValue, for: sessionID) }
    }

    private var trimmed: String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var shape: RoundedRectangle {
        .rect(cornerRadius: isCompact ? 36 : 26)
    }

    private var isCompact: Bool {
        !isFocused && trimmed.isEmpty && images.isEmpty && accessoryIsEmpty
    }

    /// Accessory content can include pending approvals or requests; keep those visible.
    private var accessoryIsEmpty: Bool {
        !keepsExpanded
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            accessory()
            if isFocused && trimmed.hasPrefix("/") { commandSuggestions }
            if !images.isEmpty { imagePreviews }
            ZStack(alignment: .bottom) {
                messageField
                    .padding(.leading, isCompact ? buttonSize + 8 : 8)
                    .padding(.trailing, isCompact ? buttonSize + 8 : 8)
                    .padding(.bottom, isCompact ? 0 : buttonSize + 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .frame(minHeight: isCompact ? buttonSize : buttonSize * 2 + 8)

                HStack(spacing: 8) {
                    chatActionsButton
                    Spacer(minLength: 0)
                    if !isCompact { controls().disabled(isSending || isImporting) }
                    trailingAction
                }
            }
            .padding(8)
        }
        .contentShape(shape)
        .glassSurface(in: shape, interactive: true)
        .sensoryFeedback(.selection, trigger: isFocused)
        .animation(reduceMotion ? nil : .snappy(duration: 0.24), value: isCompact)
        .sensoryFeedback(.impact(weight: .light), trigger: sendCount)
        .sensoryFeedback(.impact(weight: .medium, intensity: 0.8), trigger: stopCount)
        .onChange(of: showsActions) { _, presented in
            if !presented { performNextAction() }
        }
        .sensoryFeedback(.selection, trigger: showsActions)
        .copyConfirmation("Thread ID Copied", trigger: copiedIDCount, alignment: .top)
        .sensoryFeedback(.success, trigger: images.count) { old, new in new > old }
        .sheet(isPresented: $showsFolder) { NewSessionView() }
        .fullScreenCover(isPresented: $showsCamera) {
            CameraPhotoPicker { result in
                showsCamera = false
                switch result {
                case .success(let data): if let data { importCameraPhoto(data) }
                case .failure(let error): store.notice = error.localizedDescription
                }
            }
            .ignoresSafeArea()
        }
        .fileImporter(isPresented: $showsFiles, allowedContentTypes: [.image], allowsMultipleSelection: true, onCompletion: importFiles)
        .photosPicker(isPresented: $showsPhotos, selection: $photoItems, maxSelectionCount: max(1, 4 - images.count), matching: .images)
        .onChange(of: photoItems) { _, items in importPhotos(items) }
        #if DEBUG
        .task {
            let options = LaunchOptions.current
            if drafts.values[sessionID] == nil, options.demoSessionID == sessionID, let draft = options.composerDraft { text = draft }
            if options.focusesComposer {
                try? await Task.sleep(for: .milliseconds(600))
                isFocused = true
            }
        }
        #endif
    }

    private var chatActionsButton: some View {
        Button {
            showsActions.toggle()
        } label: {
            Image(systemName: "plus")
                .font(.title3)
                .frame(width: buttonSize, height: buttonSize)
                .background(.fill.tertiary, in: .circle)
        }
        .buttonStyle(ComposerPressStyle()).accessibilityLabel("Thread actions")
        .disabled(isSending || isImporting)
        .anchorPreference(key: ChatActionsAnchor.self, value: .bounds) { $0 }
    }

    @ViewBuilder
    private var trailingAction: some View {
        if isSending || isImporting || store.isUploadingImages {
            ProgressView().frame(width: buttonSize, height: buttonSize).accessibilityLabel("Preparing message")
        } else if isRunning && trimmed.isEmpty && images.isEmpty {
            stopButton
        } else if isRunning && !trimmed.hasPrefix("/") {
            queueButton
        } else {
            sendButton
        }
    }

    private var messageField: some View {
        TextField(isRunning ? "Queue a message" : "Work with ChatGPT", text: Binding(get: { text }, set: { text = $0 }), axis: .vertical)
            .lineLimit(1...6)
            .font(.body)
            // Baseline-aligned: centring a Text over the field's editor leaves it a
            // fraction of a point off the typed glyphs.
            .overlay(alignment: Alignment(horizontal: .leading, vertical: .firstTextBaseline)) {
                if let completionSuffix {
                    // One shaped string in the field's own font: the typed text, drawn
                    // clear, places the remaining suffix exactly after the caret.
                    Text(ghostCompletion(suffix: completionSuffix))
                        .font(.body).lineLimit(1)
                        .allowsHitTesting(false).accessibilityHidden(true)
                }
            }
            .focused($isFocused)
            .disabled(isSending || isImporting)
            .frame(maxWidth: .infinity, minHeight: buttonSize)
            .accessibilityLabel(isRunning ? "Message to queue" : "Message")
            .animation(reduceMotion ? nil : .snappy(duration: 0.2), value: isRunning)
    }

    private var matchingCommands: [SessionCommandChoice] {
        CommandSuggestions.matches(trimmed, in: commands)
    }

    private var completionSuffix: String? {
        guard isFocused, text == trimmed else { return nil }
        return CommandSuggestions.completionSuffix(for: text, in: commands)
    }

    private func ghostCompletion(suffix: String) -> AttributedString {
        var typed = AttributedString(text)
        typed.foregroundColor = .clear
        var rest = AttributedString(suffix)
        rest.foregroundColor = Color.secondary.opacity(0.6)
        return typed + rest
    }

    private func commandText(_ choice: SessionCommandChoice) -> String {
        CommandSuggestions.commandText(choice)
    }

    private func applyCommand(_ choice: SessionCommandChoice) {
        text = choice.insertion.isEmpty ? commandText(choice) : choice.insertion
    }

    /// Rows have one fixed, text-scaled height so the panel shows whole rows only: no row
    /// is cut mid-text against the field, and scrolling rests on row boundaries.
    private var commandSuggestions: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !matchingCommands.isEmpty {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(matchingCommands.enumerated()), id: \.element.id) { index, choice in
                            Button { applyCommand(choice) } label: {
                                commandRow(choice)
                            }
                            .buttonStyle(CommandRowStyle())
                            .overlay(alignment: .top) {
                                if index > 0 { Divider().padding(.leading, 16) }
                            }
                            .accessibilityLabel(commandText(choice))
                            .accessibilityHint(choice.description)
                        }
                    }
                    .scrollTargetLayout()
                }
                .frame(height: CGFloat(min(matchingCommands.count, Self.visibleCommandRows)) * commandRowHeight)
                .scrollTargetBehavior(.viewAligned)
                .scrollBounceBehavior(.basedOnSize)
                .scrollDismissesKeyboard(.never)
                .clipShape(.rect)
                .padding(.top, 4)
                .accessibilityIdentifier("commandSuggestions")
                Divider().padding(.horizontal, 14)
            }
        }
    }

    private static var visibleCommandRows: Int { 3 }

    private func commandRow(_ choice: SessionCommandChoice) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 1) {
                Text(commandText(choice))
                    .font(.body.weight(.medium))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                // Always two lines tall, so every row has the same height.
                Text(choice.description.isEmpty ? " " : choice.description)
                    .font(.footnote).foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            if choice.action == "terminal" {
                Image(systemName: "desktopcomputer")
                    .font(.footnote).foregroundStyle(.secondary)
                    .accessibilityLabel("Runs in the Mac terminal")
            }
        }
        .padding(.horizontal, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: commandRowHeight)
        .contentShape(.rect)
    }

    private var commands: [SessionCommandChoice] {
        if let commands = store.attached?.commands, !commands.isEmpty { return commands }
        return [
            ("/help", "All commands", "execute"), ("/model", "Choose a model", "model_picker"),
            ("/new", "New thread", "new_session"), ("/history", "Threads", "session_picker"),
            ("/copy", "Copy last response", "copy_response"), ("/effort", "Thinking effort", "execute"),
            ("/cancel", "Stop the current task", "execute"), ("/status", "Thread status", "execute"),
            ("/context", "Context window", "execute"), ("/usage", "Token usage", "execute"),
            ("/tasks", "Background tasks", "execute"), ("/mcp", "Configured MCP servers", "execute"),
            ("/title", "Rename thread", "execute"), ("/info", "About RustCode", "execute"),
            ("/perf", "Performance", "execute")
        ].map { SessionCommandChoice(name: $0.0, description: $0.1, insertion: $0.0, action: $0.2) }
    }

    private var sendButton: some View {
        Button {
            send(.automatic)
        } label: {
            circle("arrow.up", isProminent: !trimmed.isEmpty || !images.isEmpty)
        }
        .buttonStyle(ComposerPressStyle())
        .disabled((trimmed.isEmpty && images.isEmpty) || !isEnabled || isSending || isImporting)
        .accessibilityLabel(trimmed.hasPrefix("/") ? "Run command" : "Send message")
    }

    /// While a turn runs, a tap queues; the menu offers steering instead.
    private var queueButton: some View {
        Menu {
            Button("Stop the running turn", role: .destructive) { Task { await store.cancelTurn() } }
            Button("Queue for Next Turn", systemImage: "text.line.last.and.arrowtriangle.forward") {
                send(.queue)
            }
            if canSteer {
                Button("Steer Running Turn", systemImage: "arrow.triangle.turn.up.right.diamond") {
                    send(.steer)
                }
            }
        } label: {
            circle("arrow.up", isProminent: true)
        } primaryAction: {
            send(.queue)
        }
        .buttonStyle(ComposerPressStyle())
        .disabled(!isEnabled)
        .accessibilityLabel("Queue message")
        .accessibilityHint("Touch and hold for steering")
    }

    private var stopButton: some View {
        Button {
            stopCount += 1
            Task { await store.cancelTurn() }
        } label: {
            if store.hasPendingCancellation {
                ProgressView().frame(width: buttonSize, height: buttonSize)
            } else {
                circle("stop.fill", fill: AnyShapeStyle(.red), foreground: .white)
            }
        }
        .buttonStyle(ComposerPressStyle())
        .disabled(!isEnabled || store.hasPendingCancellation)
        .accessibilityLabel(store.hasPendingCancellation ? "Stopping the running turn" : "Stop the running turn")
    }

    private func circle(_ symbol: String, isProminent: Bool) -> some View {
        circle(
            symbol,
            fill: isProminent ? AnyShapeStyle(.brandFill) : AnyShapeStyle(.fill.secondary),
            foreground: isProminent ? .white : .secondary
        )
    }

    private func circle(_ symbol: String, fill: AnyShapeStyle, foreground: Color) -> some View {
        Image(systemName: symbol)
            .font(.title3.weight(.medium))
            .foregroundStyle(foreground)
            .frame(width: buttonSize, height: buttonSize)
            .background(fill, in: .circle)
            .contentShape(.rect)
    }

    private var imagePreviews: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(images) { image in
                    PromptImageThumbnail(image: image)
                            .overlay(alignment: .topTrailing) {
                                Button { images.removeAll { $0.id == image.id } } label: {
                                    Image(systemName: "xmark.circle.fill").symbolRenderingMode(.palette)
                                        .font(.system(size: 23, weight: .medium))
                                        .foregroundStyle(.white, .black.opacity(0.75))
                                        .frame(width: 44, height: 44)
                                }
                                .buttonStyle(.plain).accessibilityLabel("Remove image")
                                .disabled(isSending || isImporting)
                            }
                            .accessibilityElement(children: .contain)
                            .accessibilityLabel("Attached image")
                }
            }
            .padding(.horizontal, 14).padding(.top, 10)
        }
        .scrollIndicators(.hidden)
    }

    private func performNextAction() {
        guard let action = nextAction else { return }
        nextAction = nil
        switch action {
        case .newSession:
            Task { await store.createSession(directory: nil) }
        case .folder:
            showsFolder = true
        case .copySessionID:
            Clipboard.copy(sessionID)
            copiedIDCount += 1
        case .commands:
            if text.isEmpty { text = "/" }
            isFocused = true
        case .dismissKeyboard:
            isFocused = false
        case .stop:
            Task { await store.cancelTurn() }
        case .photos:
            guard canAttachImage() else { return }
            showsPhotos = true
        case .files:
            guard canAttachImage() else { return }
            showsFiles = true
        case .camera:
            guard canAttachImage() else { return }
            Task { await openCamera() }
        }
    }

    private func canAttachImage() -> Bool {
        guard store.supportsImageUpload else {
            store.notice = "Update RustCode on your Mac to attach images to this thread."
            return false
        }
        guard images.count < 4 else {
            store.notice = "You can attach up to four images to a message."
            return false
        }
        return true
    }

    private func openCamera() async {
        guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
            store.notice = "The camera is unavailable on this device. Choose Photos or Files to attach an image."
            return
        }
        let status = AVCaptureDevice.authorizationStatus(for: .video)
        let allowed: Bool
        if status == .notDetermined {
            allowed = await AVCaptureDevice.requestAccess(for: .video)
        } else {
            allowed = status == .authorized
        }
        guard allowed else {
            store.notice = "Allow camera access in Settings to take a photo, or choose Photos or Files."
            return
        }
        showsCamera = true
    }

    private func importCameraPhoto(_ data: Data) {
        guard !isImporting, images.count < 4 else { return }
        isImporting = true
        Task {
            defer { isImporting = false }
            do {
                let image = try await Task.detached { try PromptImageImport.prepare(data) }.value
                images.append(image)
            } catch { store.notice = error.localizedDescription }
        }
    }

    private func importFiles(_ result: Result<[URL], Error>) {
        guard !isImporting else { return }
        do {
            let urls = try result.get()
            guard !urls.isEmpty else { return }
            let remaining = 4 - images.count
            isImporting = true
            Task {
                defer { isImporting = false }
                do {
                    let imported = try await Task.detached {
                        try urls.prefix(remaining).map { url in
                            let scoped = url.startAccessingSecurityScopedResource()
                            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                            guard size <= 40 * 1024 * 1024 else { throw PromptImageImport.ImportError.tooLarge }
                            return try PromptImageImport.prepare(Data(contentsOf: url, options: .mappedIfSafe))
                        }
                    }.value
                    images.append(contentsOf: imported)
                    if urls.count > remaining {
                        store.notice = "You can attach up to four images. Only the first \(remaining) selected files were added."
                    }
                } catch { store.notice = error.localizedDescription }
            }
        } catch {
            if (error as NSError).code != NSUserCancelledError { store.notice = error.localizedDescription }
        }
    }

    private func importPhotos(_ items: [PhotosPickerItem]) {
        guard !items.isEmpty, !isImporting else { return }
        isImporting = true
        Task {
            defer { photoItems = []; isImporting = false }
            var imported: [PromptImage] = []
            do {
                for item in items.prefix(4 - images.count) {
                    guard let data = try await item.loadTransferable(type: Data.self) else { throw PromptImageImport.ImportError.unreadable }
                    let image = try await Task.detached { try PromptImageImport.prepare(data) }.value
                    imported.append(image)
                }
                images.append(contentsOf: imported)
            } catch { store.notice = error.localizedDescription }
        }
    }

    private func send(_ mode: SendMode) {
        let message = trimmed
        guard !isSending, !message.isEmpty || !images.isEmpty else { return }
        guard !message.hasPrefix("/") || images.isEmpty else {
            store.notice = "Send attached images with a message instead of a slash command."
            return
        }
        let outgoingImages = images
        isFocused = false
        // Commit UIKit's resignation before disabling the field; otherwise SwiftUI can
        // reattach its editor as the turn changes from Message to Queue a message.
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        isSending = true
        sendCount += 1
        Task {
            defer { isSending = false }
            guard store.selectedSessionID == sessionID else { return }
            let accepted = message.hasPrefix("/")
                ? await runCommand(message)
                : await store.send(message, images: outgoingImages, mode: mode)
            if accepted {
                if store.selectedSessionID == sessionID && !isFocused {
                    UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
                }
                if !outgoingImages.isEmpty {
                    try? await Task.detached(priority: .utility) {
                        try SentImageCache.shared.save(outgoingImages)
                    }.value
                }
                drafts.clearIfMatching(text: message, images: outgoingImages, sessionID: sessionID)
            }
        }
    }
}

/// A full-width suggestion row that highlights while pressed.
private struct CommandRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(configuration.isPressed ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear))
    }
}

struct ComposerPressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .brightness(configuration.isPressed ? 0.08 : 0)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.14), value: configuration.isPressed)
    }
}
