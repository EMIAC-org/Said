import XCTest

/// Drives the entire production flow against the LIVE backend: real signup →
/// every onboarding step → all five main tabs. Captures a screenshot at each
/// stop so the flow can be reviewed visually. This is verification, not a mock.
final class OnboardingWalkthroughTests: XCTestCase {
    let app = XCUIApplication()

    override func setUp() {
        continueAfterFailure = false
        app.launch()
    }

    func snap(_ name: String) {
        let shot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testFullWalkthrough() {
        // 1. Welcome
        let getStarted = app.buttons["Get started"]
        XCTAssertTrue(getStarted.waitForExistence(timeout: 10), "Welcome screen should appear")
        snap("01-welcome")
        getStarted.tap()

        // 2. Account — real signup with a unique email
        let email = app.textFields.firstMatch
        XCTAssertTrue(email.waitForExistence(timeout: 10), "Account screen should appear")
        snap("02-account")
        email.tap()
        let unique = "ios-uitest-\(Int(Date().timeIntervalSince1970))@airnote.test"
        email.typeText(unique)
        let password = app.secureTextFields.firstMatch
        password.tap()
        password.typeText("airnoteUITEST123")
        // Default is signup; the "Create account" button is the primary CTA.
        let createAccount = app.buttons["Create account"]
        XCTAssertTrue(createAccount.waitForExistence(timeout: 5))
        createAccount.tap()

        // 3. Privacy — appears after the real signup round-trip succeeds
        let understand = app.switches.firstMatch
        XCTAssertTrue(understand.waitForExistence(timeout: 20), "Privacy step should appear after signup")
        snap("03-privacy")
        understand.tap()
        tapPrimary("Continue", fallback: nil)

        // 4. Microphone — pre-granted in the simulator, so "Continue" is shown
        XCTAssertTrue(waitForAny(["Continue", "Allow microphone"], timeout: 10), "Microphone step should appear")
        snap("04-microphone")
        if app.buttons["Allow microphone"].exists { app.buttons["Allow microphone"].tap() }
        tapPrimary("Continue", fallback: "Continue anyway")

        // 5. Keyboard
        XCTAssertTrue(waitForAny(["I'll do this later", "Continue"], timeout: 10), "Keyboard step should appear")
        snap("05-keyboard")
        tapPrimary("I'll do this later", fallback: "Continue")

        // 5b. Voice keys (BYOK) — skip in the test (no real provider keys)
        XCTAssertTrue(waitForAny(["I'll add keys later", "Continue"], timeout: 10), "Voice keys step should appear")
        snap("05b-voice-keys")
        tapPrimary("I'll add keys later", fallback: "Continue")

        // 6. First dictation — test account has no provider creds, so "Continue"
        XCTAssertTrue(waitForAny(["Continue", "Try a dictation", "Skip for now"], timeout: 10), "First dictation step")
        snap("06-first-dictation")
        tapPrimary("Continue", fallback: "Skip for now")

        // 7. Personalize
        let finish = app.buttons["Start using AirNote"]
        XCTAssertTrue(finish.waitForExistence(timeout: 10), "Personalize step should appear")
        snap("07-personalize")
        finish.tap()

        // 8. Main app — Dashboard, then every tab
        XCTAssertTrue(app.tabBars.buttons["Home"].waitForExistence(timeout: 15), "Main tab bar should appear")
        snap("08-dashboard")

        tapTab("History"); snap("09-history")
        tapTab("Words"); snap("10-vocabulary")
        tapTab("Insights"); snap("11-insights")
        tapTab("Settings"); snap("12-settings")

        // Sanity: settings shows the signed-in account
        XCTAssertTrue(app.staticTexts[unique].waitForExistence(timeout: 5), "Settings should show the account email")
    }

    // MARK: Helpers

    private func tapPrimary(_ label: String, fallback: String?) {
        if app.buttons[label].waitForExistence(timeout: 6) {
            app.buttons[label].tap()
        } else if let fallback, app.buttons[fallback].exists {
            app.buttons[fallback].tap()
        }
    }

    private func tapTab(_ name: String) {
        let tab = app.tabBars.buttons[name]
        XCTAssertTrue(tab.waitForExistence(timeout: 8), "Tab \(name) should exist")
        tab.tap()
    }

    private func waitForAny(_ labels: [String], timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            for label in labels where app.buttons[label].exists { return true }
            usleep(200_000)
        }
        return false
    }
}
