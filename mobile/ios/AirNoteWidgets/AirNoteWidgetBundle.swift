import SwiftUI
import WidgetKit

@main
struct AirNoteWidgetBundle: WidgetBundle {
    var body: some Widget {
        if #available(iOS 16.1, *) {
            DictationSessionLiveActivity()
        }
    }
}
