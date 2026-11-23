#if os(macOS)
import SwiftUI

/// Native translation of Design/Face's named vector layers, in 1024 master units.
public struct FaceArtwork: View {
    public let parameters: FaceParameters
    public init(parameters: FaceParameters) { self.parameters = parameters }
    public var body: some View {
        Canvas { context, size in
            let scale = min(size.width, size.height) / 1024
            context.translateBy(x: (size.width - 1024 * scale) / 2, y: (size.height - 1024 * scale) / 2)
            context.scaleBy(x: scale, y: scale)
            context.translateBy(x: 512, y: 512)
            context.scaleBy(x: parameters.headScale, y: parameters.headScale)
            context.translateBy(x: -512, y: -512)
            let ink = Color(red: 16/255, green: 25/255, blue: 23/255)
            context.fill(Path(roundedRect: CGRect(x: 212, y: 212, width: 600, height: 600), cornerRadius: 260), with: .color(Color(red: 135/255, green: 239/255, blue: 188/255)))
            for x in [360.0, 606.0] {
                context.fill(Path(roundedRect: CGRect(x: x, y: 430 - parameters.eyeHeight / 2, width: 58, height: parameters.eyeHeight), cornerRadius: min(29, parameters.eyeHeight / 2)), with: .color(ink))
            }
            context.fill(Path(roundedRect: CGRect(x: 428, y: 570, width: 168, height: parameters.mouthHeight), cornerRadius: parameters.mouthHeight / 2), with: .color(ink))
            var smile = Path()
            smile.move(to: CGPoint(x: 420, y: 600))
            smile.addQuadCurve(to: CGPoint(x: 604, y: 600), control: CGPoint(x: 512, y: 690))
            context.stroke(smile, with: .color(ink.opacity(parameters.happy)), style: StrokeStyle(lineWidth: 20, lineCap: .round))
            var brows = Path()
            brows.move(to: CGPoint(x: 350, y: 340)); brows.addLine(to: CGPoint(x: 430, y: 370))
            brows.move(to: CGPoint(x: 594, y: 370)); brows.addLine(to: CGPoint(x: 674, y: 340))
            context.stroke(brows, with: .color(ink.opacity(parameters.concerned)), style: StrokeStyle(lineWidth: 18, lineCap: .round))
        }.accessibilityHidden(true)
    }
}
#endif
