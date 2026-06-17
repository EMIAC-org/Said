import UIKit

final class ErrorDrawer: UIView {
    init(message: String) {
        super.init(frame: .zero)
        backgroundColor = KeyboardTheme.warning.withAlphaComponent(0.14)
        layer.cornerRadius = KeyboardTheme.radius
        layer.borderColor = KeyboardTheme.warning.withAlphaComponent(0.28).cgColor
        layer.borderWidth = 1

        let icon = UIImageView(image: UIImage(systemName: "exclamationmark.triangle.fill"))
        icon.tintColor = KeyboardTheme.warning
        icon.setContentHuggingPriority(.required, for: .horizontal)

        let label = UILabel()
        label.text = message
        label.font = .preferredFont(forTextStyle: .footnote)
        label.textColor = .label
        label.numberOfLines = 0

        let stack = UIStackView(arrangedSubviews: [icon, label])
        stack.axis = .horizontal
        stack.alignment = .top
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8)
        ])

        accessibilityLabel = message
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
    }
}
