import UIKit

final class NextKeyboardButton: UIButton {
    init() {
        super.init(frame: .zero)
        var config = UIButton.Configuration.plain()
        config.image = UIImage(systemName: "globe")
        config.baseForegroundColor = .label
        config.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 10, bottom: 8, trailing: 10)
        configuration = config
        accessibilityLabel = "Next keyboard"
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
    }
}
