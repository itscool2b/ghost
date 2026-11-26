import XCTest

/// Every theme, light and dark, at the usual text size and a large one, on the screens people see
/// most: the Inbox, a chat and its changes. The screenshots are kept in the test results. Run on
/// purpose only (`TEST_RUNNER_GHOST_SCREENSHOTS=1`, or the iPhone app workflow's screenshots input):
/// forty launches take minutes. `TEST_RUNNER_GHOST_SCREENSHOT_THEMES=claude,codex` narrows the themes.
final class ScreenshotTests: XCTestCase {
    static let themes = [
        "aura", "claude", "codex", "grok", "pi", "antigravity", "cursor", "opencode", "oh_my_pi", "hermes",
    ]
    static let sizes = [("usual", "UICTContentSizeCategoryL"), ("large", "UICTContentSizeCategoryAccessibilityL")]

    override func setUp() {
        continueAfterFailure = true
    }

    func testEveryThemeOnTheMainScreens() throws {
        let environment = ProcessInfo.processInfo.environment
        try XCTSkipUnless(environment["GHOST_SCREENSHOTS"] == "1", "Screenshots run on purpose: GHOST_SCREENSHOTS=1.")
        let themes = environment["GHOST_SCREENSHOT_THEMES"].map { $0.split(separator: ",").map(String.init) } ?? Self.themes
        for theme in themes {
            for appearance in ["light", "dark"] {
                for (size, category) in Self.sizes {
                    let app = XCUIApplication()
                    app.launchArguments = [
                        "-GhostBackend", "mock", "-GhostMockPaired", "-themeOverride", theme, "-appearance", appearance,
                        "-UIPreferredContentSizeCategoryName", category,
                    ]
                    app.launch()
                    let name = "\(theme) \(appearance) \(size)"
                    guard app.staticTexts["Needs you"].waitForExistence(timeout: 30) else {
                        XCTFail("\(name): the Inbox did not show")
                        app.terminate()
                        continue
                    }
                    sleep(1)
                    shoot(app, "\(name): Inbox")
                    app.swipeUp()
                    let row = app.buttons.containing(NSPredicate(format: "label CONTAINS 'Fix the login form'")).firstMatch
                    if row.waitForExistence(timeout: 5) {
                        row.tap()
                        if app.staticTexts["Why does the login form submit empty emails?"].waitForExistence(timeout: 10) {
                            sleep(1)
                            shoot(app, "\(name): chat")
                            let strip = app.buttons.containing(NSPredicate(format: "label BEGINSWITH 'Changes:'"))
                                .firstMatch
                            if strip.waitForExistence(timeout: 5) {
                                strip.tap()
                                if app.staticTexts["Pull request #12"].waitForExistence(timeout: 5) {
                                    shoot(app, "\(name): changes")
                                }
                            }
                        }
                    }
                    app.terminate()
                }
            }
        }
    }

    private func shoot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
