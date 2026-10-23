import AppKit
import SwiftUI

// The palette of the iOS app, so both clients read as one product:
//
// - `brand`: the logo's slate blue, for text and symbols. Lighter in dark mode.
// - `brandFill`: the same hue as a fill that carries a white label in both appearances.
// - `attention`: something is waiting for the user (an approval, a question, an error).
//   Amber, so it never reads as the brand accent.
extension Color {
    static let brand = Color.dynamic(light: 0x435C6E, dark: 0x7FA3BD)
    static let brandFill = Color.dynamic(light: 0x435C6E, dark: 0x4F6D83)
    static let attention = Color.dynamic(light: 0x8A5A00, dark: 0xFFD60A)
    /// The window's two planes: a gray sidebar beside a lighter (or, in dark
    /// mode, deeper) chat, told apart by tone instead of a divider line.
    static let sidebarBackground = Color.dynamic(light: 0xEEEEF0, dark: 0x2A2A2D)
    static let chatBackground = Color.dynamic(light: 0xFFFFFF, dark: 0x1E1E20)

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat(hex >> 16 & 0xFF) / 255,
                           green: CGFloat(hex >> 8 & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
        })
    }
}

extension View {
    /// A panel that floats over the transcript: a translucent material and a soft
    /// shadow, with no outline.
    func floatingSurface(in shape: some Shape) -> some View {
        background {
            shape
                .fill(.regularMaterial)
                .shadow(color: .black.opacity(0.10), radius: 10, y: 3)
        }
    }

    /// Liquid Glass on macOS 26 and later, the floating material before that.
    /// The SDK check keeps the package building with Xcode releases older than 26.
    @ViewBuilder
    func glassSurface(in shape: some Shape) -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26, *) {
            glassEffect(.regular, in: shape)
        } else {
            floatingSurface(in: shape)
        }
        #else
        floatingSurface(in: shape)
        #endif
    }

    /// Liquid Glass that answers the pointer, for a floating button: it
    /// lights under the pointer and flexes when pressed. A material capsule
    /// before macOS 26.
    @ViewBuilder
    func interactiveGlassSurface(in shape: some Shape) -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26, *) {
            glassEffect(.regular.interactive(), in: shape)
        } else {
            floatingSurface(in: shape)
        }
        #else
        floatingSurface(in: shape)
        #endif
    }

    /// A card in the transcript: a quiet fill with an optional coloured hairline.
    func card(cornerRadius: CGFloat = 14, outline: Color? = nil) -> some View {
        padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.fill.quaternary, in: .rect(cornerRadius: cornerRadius))
            .overlay {
                if let outline {
                    RoundedRectangle(cornerRadius: cornerRadius).strokeBorder(outline.opacity(0.45), lineWidth: 1)
                }
            }
    }

    /// The main action of a card, filled with the brand colour.
    func brandProminent() -> some View {
        buttonStyle(.borderedProminent).tint(.brandFill)
    }
}

extension View {
    /// The pointing hand while the pointer is over this, so whatever opens or
    /// toggles on a click says so before it is pressed.
    func pointingHandCursor() -> some View {
        modifier(PointingHandCursor())
    }

    /// A pressable row or chip in the chat: a quiet fill under the pointer
    /// and the pointing hand, as the toolbar's `ChromeButton` answers it.
    func hoverHighlight(cornerRadius: CGFloat = 8, padding: CGFloat = 0) -> some View {
        modifier(HoverHighlight(cornerRadius: cornerRadius, padding: padding))
    }
}

private struct PointingHandCursor: ViewModifier {
    @State private var pushed = false

    func body(content: Content) -> some View {
        content
            .onHover { inside in
                guard inside != pushed else { return }
                if inside { NSCursor.pointingHand.push() } else { NSCursor.pop() }
                pushed = inside
            }
            // A view that goes while under the pointer would leave the hand behind.
            .onDisappear {
                if pushed { NSCursor.pop() }
                pushed = false
            }
    }
}

private struct HoverHighlight: ViewModifier {
    let cornerRadius: CGFloat
    /// How far the fill reaches past the content, without moving it.
    let padding: CGFloat
    @State private var isHovered = false

    func body(content: Content) -> some View {
        content
            .background {
                RoundedRectangle(cornerRadius: cornerRadius)
                    .fill(.fill.tertiary)
                    .padding(-padding)
                    .opacity(isHovered ? 1 : 0)
            }
            .onHover { isHovered = $0 }
            .animation(.easeOut(duration: 0.12), value: isHovered)
            .pointingHandCursor()
    }
}

enum Clipboard {
    @MainActor
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
