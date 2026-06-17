import Foundation
import Combine

public final class EventQueue: ObservableObject {
    @Published public private(set) var pending: [MobileEvent] = []

    public init() {}

    public func enqueue(_ event: MobileEvent) {
        pending.append(event)
    }

    public func drain() -> [MobileEvent] {
        let events = pending
        pending.removeAll()
        return events
    }
}
