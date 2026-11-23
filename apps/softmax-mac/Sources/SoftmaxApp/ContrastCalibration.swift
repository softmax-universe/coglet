import SwiftUI

/// Isolated, test-only experiment: distinguish raster sampling from token inheritance.
/// Never reachable outside --ui-testing; remove after diagnosing the runner audit.
struct ContrastCalibrationView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Appearance").font(.system(size: 13)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-13-regular")
            Text("Appearance").font(.system(size: 13)).foregroundStyle(.black).accessibilityIdentifier("black-13-regular")
            Text("Appearance").font(.system(size: 13, weight: .medium)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-13-medium")
            Text("Appearance").font(.system(size: 14)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-14-regular")
            Text("Appearance").font(.system(size: 15)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-15-regular")
            Text("Appearance").font(.system(size: 17)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-17-regular")
            Text("Appearance").font(.body).foregroundStyle(.primary).accessibilityIdentifier("native-body-primary")
            Text("Appearance").font(.system(size: 13)).foregroundStyle(SoftmaxDesignTokens.Light.ink).background(.white).accessibilityIdentifier("token-13-leaf-background")
            Text("Appearance").font(.system(size: 13, weight: .semibold)).foregroundStyle(SoftmaxDesignTokens.Light.ink).accessibilityIdentifier("token-13-semibold")
        }
        .padding(24).frame(width: 760, height: 540, alignment: .topLeading)
        .background(.white).environment(\.colorScheme, .light)
    }
}
