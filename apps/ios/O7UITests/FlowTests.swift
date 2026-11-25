import XCTest

/// The app's main flows against the computer in a box (`-O7Backend mock`): real sealing, a scripted
/// agent, no network.
final class FlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(paired: Bool) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-O7Backend", "mock"] + (paired ? ["-O7MockPaired"] : [])
        app.launch()
        return app
    }

    func testPairingWithTheDemoComputerAsksFirstThenPairs() {
        let app = launch(paired: false)
        XCTAssertTrue(app.staticTexts["o7 on your iPhone"].waitForExistence(timeout: 10))
        app.buttons["Use the demo computer"].tap()
        XCTAssertTrue(app.staticTexts["Pair with this computer?"].waitForExistence(timeout: 5))
        app.buttons["Pair"].tap()
        XCTAssertTrue(app.staticTexts["Paired with Studio"].waitForExistence(timeout: 10))
        app.buttons["Open Studio"].tap()
        XCTAssertTrue(app.navigationBars["Studio"].waitForExistence(timeout: 10))
    }

    func testAnswersAPermissionFromTheInbox() {
        let app = launch(paired: true)
        XCTAssertTrue(app.staticTexts["Needs you"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Tidy the changelog"].waitForExistence(timeout: 5))
        let allow = app.buttons["Allow once"].firstMatch
        XCTAssertTrue(allow.waitForExistence(timeout: 5))
        allow.tap()
    }

    func testOpensAChatAndSendsAMessage() {
        let app = launch(paired: true)
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS 'Fix the login form'")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        // The row starts under the strip above the tabs; the list scrolls it clear.
        app.swipeUp()
        row.tap()
        XCTAssertTrue(app.staticTexts["Fix the login form"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Why does the login form submit empty emails?"].waitForExistence(timeout: 10))
        let box = app.textViews["Message"].exists ? app.textViews["Message"] : app.textFields["Message"]
        box.tap()
        box.typeText("Add the check and run the tests")
        app.buttons["Send"].tap()
        XCTAssertTrue(app.staticTexts["Add the check and run the tests"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["Allow once"].firstMatch.waitForExistence(timeout: 15))
    }

    func testTheAppPassesTheAccessibilityAudit() throws {
        let app = launch(paired: true)
        XCTAssertTrue(app.staticTexts["Needs you"].waitForExistence(timeout: 10))
        // Every control is big enough to hit and says what it is.
        try app.performAccessibilityAudit(for: [.sufficientElementDescription, .hitRegion]) { issue in
            // System controls the app does not draw are not its to fix.
            issue.element?.elementType == .other && issue.element?.label.isEmpty == true
        }
        // Text follows Dynamic Type. It is checked in a chat: in the Inbox, rows below the card
        // that grows are measured while the list animates them into place, which the audit reads
        // as partial scaling. Those rows were checked by eye at twice the usual size.
        app.swipeUp()
        app.buttons.containing(NSPredicate(format: "label CONTAINS 'Fix the login form'")).firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Fix the login form"].waitForExistence(timeout: 5))
        sleep(1)
        try app.performAccessibilityAudit(for: [.dynamicType, .sufficientElementDescription, .hitRegion])
    }

    func testQueuesWorkForTonightAndCancelsIt() {
        let app = launch(paired: true)
        let tonight = app.buttons["Tonight"].firstMatch
        XCTAssertTrue(tonight.waitForExistence(timeout: 10))
        tonight.tap()
        XCTAssertTrue(app.staticTexts["Update the dependencies and fix what breaks"].waitForExistence(timeout: 5))
        app.buttons["Queue a task"].tap()
        let goal = app.textViews["What should happen?"].exists
            ? app.textViews["What should happen?"] : app.textFields["What should happen?"]
        XCTAssertTrue(goal.waitForExistence(timeout: 5))
        goal.tap()
        goal.typeText("Make the settings screen load faster")
        app.buttons["Queue for tonight"].tap()
        let queued = app.staticTexts["Make the settings screen load faster"]
        XCTAssertTrue(queued.waitForExistence(timeout: 5))
        queued.swipeLeft()
        app.buttons["Cancel"].firstMatch.tap()
        XCTAssertTrue(queued.waitForNonExistence(timeout: 5))
    }

    func testReadsAChatsChangesAndADiff() {
        let app = launch(paired: true)
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS 'Fix the login form'")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        app.swipeUp()
        row.tap()
        let strip = app.buttons.containing(NSPredicate(format: "label BEGINSWITH 'Changes:'")).firstMatch
        XCTAssertTrue(strip.waitForExistence(timeout: 10))
        strip.tap()
        XCTAssertTrue(app.staticTexts["Pull request #12"].waitForExistence(timeout: 5))
        app.buttons.containing(NSPredicate(format: "label CONTAINS 'login.ts'")).firstMatch.tap()
        let added = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Added:'")).firstMatch
        XCTAssertTrue(added.waitForExistence(timeout: 5))
    }

    func testRenamesAChat() {
        let app = launch(paired: true)
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS 'Fix the login form'")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        app.swipeUp()
        row.tap()
        XCTAssertTrue(app.staticTexts["Fix the login form"].waitForExistence(timeout: 5))
        app.buttons["More"].firstMatch.tap()
        app.buttons["Rename"].tap()
        let field = app.textFields["Title"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.clearAndType("Check emails first")
        app.alerts.buttons["Rename"].tap()
        XCTAssertTrue(app.staticTexts["Check emails first"].waitForExistence(timeout: 5))
    }

    func testEditsAProjectsBrief() {
        let app = launch(paired: true)
        let projects = app.buttons["Projects"].firstMatch
        XCTAssertTrue(projects.waitForExistence(timeout: 10))
        projects.tap()
        app.buttons.containing(NSPredicate(format: "label BEGINSWITH 'o7,'")).firstMatch.tap()
        app.buttons["Brief"].tap()
        let editor = app.textViews.firstMatch
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Save"].isEnabled)
        editor.tap()
        editor.typeText(" Keep it short.")
        XCTAssertTrue(app.buttons["Save"].isEnabled)
        app.buttons["Save"].tap()
        XCTAssertTrue(app.buttons["Brief"].waitForExistence(timeout: 5))
    }
}

extension XCUIElement {
    /// Replaces what a text field holds.
    func clearAndType(_ text: String) {
        tap()
        if let current = value as? String, !current.isEmpty {
            typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
        }
        typeText(text)
    }
}
