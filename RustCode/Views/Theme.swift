import SwiftUI
import UIKit

enum ChatSurface {
    /// Continue the transcript's charcoal through the status bar and home indicator.
    static let background = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark ? UIColor(white: 0.07, alpha: 1) : .systemBackground
    })
    static let sidebar = Color(uiColor: .secondarySystemBackground)
}

// Colours come from the asset catalog, which generates `.brandFill` and `.attention`:
//
// - `AccentColor`: the logo's slate blue, for text and symbols. Lighter in dark mode.
// - `BrandFill`: the same hue as a fill that carries a white label in both appearances.
// - `Attention`: something is waiting for the user (an approval, a question, a warning).
//   Amber, so it never reads as the brand accent.

extension View {
    /// Liquid Glass on iOS 26 and later, a thin material with a hairline before that.
    @ViewBuilder
    func glassSurface(in shape: some Shape, interactive: Bool = false) -> some View {
        if #available(iOS 26, *) {
            glassEffect(interactive ? .regular.interactive() : .regular, in: shape)
        } else {
            background {
                shape
                    .fill(.ultraThinMaterial)
                    .shadow(color: .black.opacity(0.12), radius: 12, y: 4)
            }
            .overlay {
                shape.stroke(.separator.opacity(0.6), lineWidth: 0.5)
            }
        }
    }

    /// A bar that floats over scrolling content. On iOS 26 the scroll view fades its
    /// content under the bar; before that it is a plain safe-area inset.
    @ViewBuilder
    func floatingBottomBar(@ViewBuilder _ bar: () -> some View) -> some View {
        if #available(iOS 26, *) {
            safeAreaBar(edge: .bottom, spacing: 0, content: bar)
        } else {
            safeAreaInset(edge: .bottom, spacing: 0, content: bar)
        }
    }

    /// The main action of a screen, filled with the brand colour.
    @ViewBuilder
    func brandProminentButtonStyle() -> some View {
        if #available(iOS 26, *) {
            buttonStyle(.glassProminent).tint(.brandFill)
        } else {
            buttonStyle(.borderedProminent).tint(.brandFill)
        }
    }

    /// A secondary action: glass on iOS 26, bordered before that.
    @ViewBuilder
    func secondaryButtonStyle() -> some View {
        if #available(iOS 26, *) {
            buttonStyle(.glass)
        } else {
            buttonStyle(.bordered)
        }
    }
}

/// Groups neighbouring glass shapes so they render and blend as one surface.
struct GlassGroup<Content: View>: View {
    var spacing: CGFloat = 8
    @ViewBuilder var content: Content

    var body: some View {
        if #available(iOS 26, *) {
            GlassEffectContainer(spacing: spacing) { content }
        } else {
            content
        }
    }
}

enum Clipboard {
    @MainActor
    static func copy(_ text: String) {
        UIPasteboard.general.string = text
    }
}
