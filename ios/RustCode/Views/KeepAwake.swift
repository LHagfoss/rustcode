import SwiftUI
import UIKit

/// Keeps the screen awake while a turn is running, so a long answer stays readable
/// without the phone dimming and locking. There is no setting: this follows the
/// assistant's activity on its own, and the screen sleeps again as soon as the turn
/// returns to idle or the app leaves the foreground.
struct KeepAwake: ViewModifier {
    var isActive: Bool
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            .onAppear { apply() }
            .onChange(of: isActive) { _, _ in apply() }
            .onChange(of: scenePhase) { _, _ in apply() }
            .onDisappear { UIApplication.shared.isIdleTimerDisabled = false }
    }

    private func apply() {
        UIApplication.shared.isIdleTimerDisabled = isActive && scenePhase == .active
    }
}

extension View {
    /// Disable the idle timer while `isActive` and the app is in the foreground.
    func keepScreenAwake(whileRunning isActive: Bool) -> some View {
        modifier(KeepAwake(isActive: isActive))
    }
}
