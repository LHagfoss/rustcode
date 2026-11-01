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
                        .clipShape(.rect(cornerRadius: 20))
                        .overlay { RoundedRectangle(cornerRadius: 20).strokeBorder(Color.primary.opacity(0.08), lineWidth: 1) }
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

/// A popover drawn by the window instead of AppKit: the composer's blurred
/// material, rounder corners and no arrow, where `.popover` is Liquid Glass
/// on macOS 26. The button marks itself with `floatingPopover`; the window's
/// root draws what is open with `floatingPopoverHost`, over everything, so
/// nothing laid out after the button covers or clips it. A click outside or
/// Esc closes it.
struct FloatingPopoverItem {
    let anchor: Anchor<CGRect>
    let content: AnyView
    let close: @MainActor () -> Void
}

struct FloatingPopoverKey: PreferenceKey {
    static let defaultValue: [FloatingPopoverItem] = []
    static func reduce(value: inout [FloatingPopoverItem], nextValue: () -> [FloatingPopoverItem]) {
        value += nextValue()
    }
}

enum FloatingPopoverMetrics {
    static let cornerRadius: CGFloat = 20
    /// The gap between the button and the panel.
    static let gap: CGFloat = 6
    /// The least room kept between the panel and the window's edge.
    static let margin: CGFloat = 10
}

extension View {
    func floatingPopover<Panel: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Panel) -> some View {
        anchorPreference(key: FloatingPopoverKey.self, value: .bounds) { anchor in
            guard isPresented.wrappedValue else { return [] }
            return [FloatingPopoverItem(anchor: anchor, content: AnyView(content()),
                                        close: { isPresented.wrappedValue = false })]
        }
    }

    func floatingPopoverHost() -> some View {
        overlayPreferenceValue(FloatingPopoverKey.self) { items in
            GeometryReader { proxy in
                if let item = items.last {
                    let button = proxy[item.anchor]
                    // Under the button, on whichever side of it has room:
                    // its leading edge in the window's left half, its
                    // trailing edge in the right.
                    let leading = button.midX < proxy.size.width / 2
                    ZStack(alignment: leading ? .topLeading : .topTrailing) {
                        Color.black.opacity(0.001)
                            .contentShape(.rect)
                            .onTapGesture { item.close() }
                        // At its ideal size, as a popover is: a scrolling
                        // list in it caps its own height.
                        item.content
                            .fixedSize()
                            .environment(\.closePanel, item.close)
                            .clipShape(.rect(cornerRadius: FloatingPopoverMetrics.cornerRadius))
                            .floatingSurface(in: .rect(cornerRadius: FloatingPopoverMetrics.cornerRadius))
                            .overlay {
                                RoundedRectangle(cornerRadius: FloatingPopoverMetrics.cornerRadius)
                                    .strokeBorder(Color.primary.opacity(0.06), lineWidth: 1)
                            }
                            .background {
                                Button("Close", action: item.close).keyboardShortcut(.cancelAction).hidden()
                            }
                            .padding(.top, button.maxY + FloatingPopoverMetrics.gap)
                            .padding(.leading, leading ? max(FloatingPopoverMetrics.margin, button.minX) : FloatingPopoverMetrics.margin)
                            .padding(.trailing, leading ? FloatingPopoverMetrics.margin
                                     : max(FloatingPopoverMetrics.margin, proxy.size.width - button.maxX))
                            .transition(.scale(scale: 0.97, anchor: leading ? .topLeading : .topTrailing).combined(with: .opacity))
                    }
                    .frame(width: proxy.size.width, height: proxy.size.height, alignment: leading ? .topLeading : .topTrailing)
                }
            }
        }
    }
}
