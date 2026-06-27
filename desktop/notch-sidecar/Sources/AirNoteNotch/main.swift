import AppKit

// Entry point. The HUD is an `.accessory` app — no Dock icon, no app menu —
// it only ever shows the borderless notch panel. The controller is retained as
// the NSApplication delegate and boots in `applicationDidFinishLaunching`.

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

let controller = NotchController()
app.delegate = controller

app.run()
