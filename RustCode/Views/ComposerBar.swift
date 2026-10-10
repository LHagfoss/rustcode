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

    @State private var sendCount = 0
    @State private var stopCount = 0
    @State private var showsActions = false
    @State private var showsFolder = false
    @State private var showsPhotos = false
    @State private var showsFiles = false
    @State private var showsCamera = false
    @State private var hadKeyboard = false
    @State private var nextAction: ChatAction?
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var isImporting = false
    @State private var isSending = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @FocusState.Binding var isFocused: Bool
    @ScaledMetric(relativeTo: .body) private var buttonSize: CGFloat = 44

    private let inset: CGFloat = 7

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
        .rect(cornerRadius: 26)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            accessory()
            if isFocused && trimmed.hasPrefix("/") { commandSuggestions }
            if !images.isEmpty { imagePreviews }
            inputRow
            HStack(spacing: 8) {
                Button {
                    hadKeyboard = isFocused
                    isFocused = false
                    showsActions = true
                } label: {
                    Image(systemName: "plus")
                        .font(.title3)
                        .frame(width: buttonSize, height: buttonSize)
                        .background(.fill.tertiary, in: .circle)
                }
                .buttonStyle(.plain).accessibilityLabel("Chat actions")
                .disabled(isSending || isImporting)
                .popover(isPresented: $showsActions, arrowEdge: .bottom) {
                    ChatActionsMenu(hadKeyboard: hadKeyboard, isRunning: isRunning) { action in
                        nextAction = action
                        showsActions = false
                    }
                    .presentationCompactAdaptation(.popover)
                    .onDisappear(perform: performNextAction)
                }
                Spacer(minLength: 0)
                controls().disabled(isSending || isImporting)
                if isSending || isImporting || store.isUploadingImages {
                    ProgressView().frame(width: buttonSize, height: buttonSize).accessibilityLabel("Preparing message")
                } else if isRunning && trimmed.isEmpty && images.isEmpty {
                    stopButton
                } else if isRunning && !trimmed.hasPrefix("/") {
                    queueButton
                } else { sendButton }
            }
            .padding(.horizontal, 8).padding(.bottom, 8)
        }
        .contentShape(shape)
        .glassSurface(in: shape)
        .sensoryFeedback(.impact(weight: .light), trigger: sendCount)
        .sensoryFeedback(.impact(weight: .medium, intensity: 0.8), trigger: stopCount)
        .sensoryFeedback(.selection, trigger: showsActions)
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

    private var inputRow: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(isRunning ? "Queue a message" : "Message", text: Binding(get: { text }, set: { text = $0 }), axis: .vertical)
                .lineLimit(1...6)
                .font(trimmed.hasPrefix("/") ? .body.monospaced() : .body)
                .overlay(alignment: .leading) {
                    if let completion {
                        (Text(text).foregroundColor(.clear) + Text(completion.dropFirst(text.count)).foregroundColor(.secondary.opacity(0.55)))
                            .font(.body.monospaced()).lineLimit(1)
                            .allowsHitTesting(false).accessibilityHidden(true)
                    }
                }
                .focused($isFocused)
                .disabled(isSending || isImporting)
                .frame(maxWidth: .infinity, minHeight: buttonSize)
                .accessibilityLabel(isRunning ? "Message to queue" : "Message")
        }
        .padding(.leading, 18)
        .padding(.trailing, inset)
        .padding(.vertical, inset)
        .animation(reduceMotion ? nil : .snappy(duration: 0.2), value: isRunning)
    }

    private var matchingCommands: [SessionCommandChoice] {
        guard !trimmed.contains(where: \.isWhitespace) else { return [] }
        return commands.filter { commandText($0).hasPrefix(trimmed.lowercased()) }
    }

    private var completion: String? {
        guard isFocused, trimmed.hasPrefix("/"), text == trimmed, trimmed.count > 1,
              let first = matchingCommands.first else { return nil }
        let value = commandText(first)
        return value.count > trimmed.count ? value : nil
    }

    private func commandText(_ choice: SessionCommandChoice) -> String {
        choice.name.hasPrefix("/") ? choice.name : "/" + choice.name
    }

    private var commandSuggestions: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !matchingCommands.isEmpty {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(matchingCommands) { choice in
                            Button {
                                text = choice.insertion.isEmpty ? commandText(choice) : choice.insertion
                            } label: {
                                HStack(spacing: 10) {
                                    Text(commandText(choice)).font(.subheadline.monospaced())
                                    Text(choice.description).font(.caption).foregroundStyle(.secondary)
                                        .lineLimit(1)
                                    Spacer(minLength: 0)
                                    Image(systemName: choice.action == "terminal" ? "desktopcomputer" : "arrow.up.left")
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 14).frame(minHeight: 44)
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(commandText(choice))
                            .accessibilityHint(choice.description)
                        }
                    }
                }
                .frame(height: min(CGFloat(matchingCommands.count) * 44, 176))
                .scrollDismissesKeyboard(.never)
                .accessibilityIdentifier("commandSuggestions")
                Divider().padding(.horizontal, 14)
            }
        }
    }

    private var commands: [SessionCommandChoice] {
        if let commands = store.attached?.commands, !commands.isEmpty { return commands }
        return [
            ("/help", "All commands", "execute"), ("/model", "Choose a model", "model_picker"),
            ("/new", "New session", "new_session"), ("/history", "Sessions", "session_picker"),
            ("/copy", "Copy last response", "copy_response"), ("/effort", "Thinking effort", "execute"),
            ("/cancel", "Stop the current task", "execute"), ("/status", "Session status", "execute"),
            ("/context", "Context window", "execute"), ("/usage", "Token usage", "execute"),
            ("/tasks", "Background tasks", "execute"), ("/mcp", "Configured MCP servers", "execute"),
            ("/title", "Rename session", "execute"), ("/info", "About RustCode", "execute"),
            ("/perf", "Performance", "execute")
        ].map { SessionCommandChoice(name: $0.0, description: $0.1, insertion: $0.0, action: $0.2) }
    }

    private var sendButton: some View {
        Button {
            send(.automatic)
        } label: {
            circle("arrow.up", isProminent: !trimmed.isEmpty || !images.isEmpty)
        }
        .buttonStyle(.plain)
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
        .buttonStyle(.plain)
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
        .buttonStyle(.plain)
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
                                        .foregroundStyle(.white, .black.opacity(0.75)).frame(width: 44, height: 44)
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
            store.notice = "Update RustCode on your Mac to attach images to this session."
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
                drafts.clearIfMatching(text: message, images: outgoingImages, sessionID: sessionID)
            }
        }
    }
}
