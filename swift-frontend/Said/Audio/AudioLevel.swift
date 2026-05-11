import Foundation

enum AudioLevelComputer {
    static func rms(_ samples: [Float]) -> Float {
        guard !samples.isEmpty else { return 0 }
        let sumOfSquares = samples.reduce(Float(0)) { $0 + $1 * $1 }
        return sqrt(sumOfSquares / Float(samples.count))
    }

    static func normalized(_ rms: Float) -> Float {
        let db = 20 * log10(max(rms, 1e-7))
        let minDb: Float = -50
        let maxDb: Float = -5
        return max(0, min(1, (db - minDb) / (maxDb - minDb)))
    }
}
