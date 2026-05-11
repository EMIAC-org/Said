import SwiftUI

struct NotchShape: Shape {
    var topCornerRadius: CGFloat
    var bottomCornerRadius: CGFloat

    init(topCornerRadius: CGFloat = 6, bottomCornerRadius: CGFloat = 14) {
        self.topCornerRadius = topCornerRadius
        self.bottomCornerRadius = bottomCornerRadius
    }

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { .init(topCornerRadius, bottomCornerRadius) }
        set {
            topCornerRadius = newValue.first
            bottomCornerRadius = newValue.second
        }
    }

    func path(in rect: CGRect) -> Path {
        let topR = topCornerRadius
        let botR = bottomCornerRadius
        let k: CGFloat = 0.552285

        var path = Path()
        // Top-left: concave quad curve (notch-style inward sweep)
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))
        path.addQuadCurve(
            to: CGPoint(x: rect.minX + topR, y: rect.minY + topR),
            control: CGPoint(x: rect.minX + topR, y: rect.minY)
        )
        // Left edge
        path.addLine(to: CGPoint(x: rect.minX + topR, y: rect.maxY - botR))
        // Bottom-left: cubic bezier for true circular arc
        path.addCurve(
            to: CGPoint(x: rect.minX + topR + botR, y: rect.maxY),
            control1: CGPoint(x: rect.minX + topR, y: rect.maxY - botR + botR * k),
            control2: CGPoint(x: rect.minX + topR + botR - botR * k, y: rect.maxY)
        )
        // Bottom edge
        path.addLine(to: CGPoint(x: rect.maxX - topR - botR, y: rect.maxY))
        // Bottom-right: cubic bezier for true circular arc
        path.addCurve(
            to: CGPoint(x: rect.maxX - topR, y: rect.maxY - botR),
            control1: CGPoint(x: rect.maxX - topR - botR + botR * k, y: rect.maxY),
            control2: CGPoint(x: rect.maxX - topR, y: rect.maxY - botR + botR * k)
        )
        // Right edge
        path.addLine(to: CGPoint(x: rect.maxX - topR, y: rect.minY + topR))
        // Top-right: concave quad curve (notch-style inward sweep)
        path.addQuadCurve(
            to: CGPoint(x: rect.maxX, y: rect.minY),
            control: CGPoint(x: rect.maxX - topR, y: rect.minY)
        )
        // Top edge
        path.addLine(to: CGPoint(x: rect.minX, y: rect.minY))
        return path
    }
}
