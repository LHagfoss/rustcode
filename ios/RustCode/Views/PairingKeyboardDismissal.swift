import SwiftUI
import UIKit

/// Let taps on the Form's headers, footers and empty space dismiss editing,
/// without swallowing field selection, buttons or nearby-Mac row actions.
struct PairingKeyboardDismissal: UIViewRepresentable {
    let dismiss: () -> Void

    func makeUIView(context: Context) -> DismissalView { DismissalView(dismiss: dismiss) }
    func updateUIView(_ view: DismissalView, context: Context) { view.dismiss = dismiss }
    static func dismantleUIView(_ view: DismissalView, coordinator: ()) { view.detach() }

    /// Mark actions that intentionally focus a field (nearby-Mac selection).
    struct PreservesFocus: UIViewRepresentable {
        func makeUIView(context: Context) -> FocusActionView { FocusActionView() }
        func updateUIView(_ view: FocusActionView, context: Context) {}
    }
    final class FocusActionView: UIView {
        override init(frame: CGRect) { super.init(frame: frame); isUserInteractionEnabled = false }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    }

    final class DismissalView: UIView, UIGestureRecognizerDelegate {
        var dismiss: () -> Void
        private lazy var tap = UITapGestureRecognizer(target: self, action: #selector(tapped))

        init(dismiss: @escaping () -> Void) {
            self.dismiss = dismiss
            super.init(frame: .zero)
            isUserInteractionEnabled = false
            tap.cancelsTouchesInView = false
            tap.delegate = self
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            detach()
            window?.addGestureRecognizer(tap)
        }
        func detach() { tap.view?.removeGestureRecognizer(tap) }
        @objc private func tapped() { dismiss() }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
            true // Dismiss editing without replacing SwiftUI button/scroll gestures.
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            guard window != nil, bounds.contains(touch.location(in: self)) else { return false }
            var target = touch.view
            while let view = target {
                if view is UIControl || view is UITextView { return false }
                if (view is UITableViewCell || view is UICollectionViewCell), preservesFocus(view) { return false }
                target = view.superview
            }
            return true
        }

        private func preservesFocus(_ view: UIView) -> Bool {
            view is FocusActionView || view.subviews.contains(where: preservesFocus)
        }
    }
}
