import AppKit
import SwiftUI

class AudioSpectrum: NSView {
    private var barLayers: [CAShapeLayer] = []
    private var barScales: [CGFloat] = []
    private var animationTimer: Timer?
    private var currentLevel: Float = 0

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        setupBars()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        wantsLayer = true
        setupBars()
    }

    private func setupBars() {
        let barWidth: CGFloat = 2
        let barCount = 4
        let spacing: CGFloat = barWidth
        let totalWidth = CGFloat(barCount) * (barWidth + spacing)
        let totalHeight: CGFloat = 14
        frame.size = CGSize(width: totalWidth, height: totalHeight)

        for i in 0..<barCount {
            let xPosition = CGFloat(i) * (barWidth + spacing)
            let barLayer = CAShapeLayer()
            barLayer.frame = CGRect(x: xPosition, y: 0, width: barWidth, height: totalHeight)
            barLayer.anchorPoint = CGPoint(x: 0.5, y: 0.5)
            barLayer.position = CGPoint(x: xPosition + barWidth / 2, y: totalHeight / 2)
            barLayer.fillColor = NSColor.white.cgColor
            barLayer.backgroundColor = NSColor.white.cgColor
            barLayer.masksToBounds = true
            let path = NSBezierPath(roundedRect: CGRect(x: 0, y: 0, width: barWidth, height: totalHeight),
                                    xRadius: barWidth / 2, yRadius: barWidth / 2)
            barLayer.path = path.cgPath
            barLayers.append(barLayer)
            barScales.append(0.2)
            layer?.addSublayer(barLayer)
        }
    }

    func setLevel(_ level: Float) {
        currentLevel = level
        if level > 0 && animationTimer == nil {
            startAnimating()
        } else if level == 0 && animationTimer != nil {
            stopAnimating()
        }
    }

    private func startAnimating() {
        guard animationTimer == nil else { return }
        animationTimer = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { [weak self] _ in
            self?.updateBars()
        }
        updateBars()
    }

    private func stopAnimating() {
        animationTimer?.invalidate()
        animationTimer = nil
        for (i, barLayer) in barLayers.enumerated() {
            barLayer.removeAllAnimations()
            barLayer.transform = CATransform3DMakeScale(1, 0.2, 1)
            barScales[i] = 0.2
        }
    }

    private func updateBars() {
        let lvl = CGFloat(currentLevel)
        for (i, barLayer) in barLayers.enumerated() {
            let currentScale = barScales[i]
            let targetScale = max(0.2, lvl * CGFloat.random(in: 0.5...1.0))
            barScales[i] = targetScale
            let animation = CABasicAnimation(keyPath: "transform.scale.y")
            animation.fromValue = currentScale
            animation.toValue = targetScale
            animation.duration = 0.15
            animation.fillMode = .forwards
            animation.isRemovedOnCompletion = false
            barLayer.add(animation, forKey: "scaleY")
        }
    }
}

struct AudioSpectrumView: NSViewRepresentable {
    var level: Float

    func makeNSView(context: Context) -> AudioSpectrum {
        AudioSpectrum()
    }

    func updateNSView(_ nsView: AudioSpectrum, context: Context) {
        nsView.setLevel(level)
    }
}
