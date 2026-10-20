import SwiftUI

private struct ClosePanelKey: EnvironmentKey {
    static let defaultValue: @MainActor () -> Void = {}
}

extension EnvironmentValues {
    /// Closes the panel a view is shown in. Panels are overlays, not sheets,
    /// so `dismiss` does not reach them.
    var closePanel: @MainActor () -> Void {
        get { self[ClosePanelKey.self] }
        set { self[ClosePanelKey.self] = newValue }
    }
}

/// A panel over the window, such as Settings: a click on the dimmed window
/// around it or Esc closes it, as a popover does. A sheet ignores clicks
/// outside it.
private struct ClickAwayPanel<Panel: View>: ViewModifier {
    let isPresented: Bool
    let close: @MainActor () -> Void
    @ViewBuilder let panel: () -> Panel

    func body(content: Content) -> some View {
        content.overlay {
            ZStack {
                if isPresented {
                    Color.black.opacity(0.22)
                        .ignoresSafeArea()
                        .contentShape(.rect)
                        .onTapGesture { close() }
                        .transition(.opacity)
                    panel()
                        .environment(\.closePanel, close)
                        .background(Color.chatBackground)
                        .clipShape(.rect(cornerRadius: 16))
                        .overlay { RoundedRectangle(cornerRadius: 16).strokeBorder(Color.primary.opacity(0.08), lineWidth: 1) }
                        .shadow(color: .black.opacity(0.22), radius: 28, y: 12)
                        .padding(28)
                        .background {
                            // Esc closes the panel from wherever focus is.
                            Button("Close", action: close).keyboardShortcut(.cancelAction).hidden()
                        }
                        .transition(.scale(scale: 0.97).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: isPresented)
        }
    }
}

extension View {
    func clickAwayPanel<Panel: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Panel) -> some View {
        modifier(ClickAwayPanel(isPresented: isPresented.wrappedValue, close: { isPresented.wrappedValue = false }, panel: content))
    }

    func clickAwayPanel<Item: Identifiable, Panel: View>(item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Panel) -> some View {
        modifier(ClickAwayPanel(isPresented: item.wrappedValue != nil, close: { item.wrappedValue = nil }) {
            if let value = item.wrappedValue { content(value) }
        })
    }
}
