import SwiftUI

/// Isolated native regression for body raster legibility, not a category exemption.
/// The 13 pt regular token failed on macOS CI even with pure black on white;
/// 14 pt regular and 13 pt medium passed the same calibration (see native-verification).
struct ContrastCalibrationView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Appearance").font(.system(size: SoftmaxDesignTokens.fontBody))
                .foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("production-body-ink")
            Text("Microphone authorization is unavailable. Check System Settings → Privacy & Security → Microphone and restart Softmax.")
                .font(.system(size: SoftmaxDesignTokens.fontBody))
                .foregroundStyle(SoftmaxDesignTokens.Light.danger).accessibilityIdentifier("production-body-danger")
            Text("Appearance").font(.body).foregroundStyle(.primary).accessibilityIdentifier("native-body-primary")
        }
        .padding(24).frame(width: 760, height: 540, alignment: .topLeading)
        .background(.white).environment(\.colorScheme, .light)
    }
}
