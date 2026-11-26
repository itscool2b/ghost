# External Windows desktop smoke

Build the frontend into `target/debug/ghost.exe` with the existing native smoke build. This
gate opens that actual executable through tauri-driver 2.1.0 and Microsoft EdgeDriver. It does
not use a Vite browser, frontend fake transport, `GHOST_SMOKE`, or an embedded pass report.

Provision EdgeDriver matching the installed WebView2 runtime, then run:

```powershell
$env:EXPECTED_WEBVIEW_VERSION = '<installed four-part WebView2 version>'
node .github/scripts/webdriver-smoke.mjs <app.exe> <tauri-driver.exe> <msedgedriver.exe> <artifacts>
```

The workflow owns driver provisioning. The harness compares the returned runtime's first three
version components with the qualified runtime and fails on a missing or incompatible version.
Do not override the runtime to avoid a platform failure. Existing WebView2 overrides are refused.

The harness creates a fresh temporary profile, home, WebView data folder and project folder with
spaces. It invokes the real `project_create` command once for that owned folder because WebView
WebDriver does not drive the operating system folder picker. All subsequent operations use W3C
element clicks, key entry, text reads and scrolling:

1. Verify one visible native window, the native bridge and embedded app origin.
2. Skip onboarding, select the demo agent and disposable project, and create a chat.
3. Type `/echo External native WebDriver hello`; check the visible prompt, reply and completed turn.
4. Type `/slow`, click Stop and check cancellation, then explicitly stop the session.
5. Reload the webview, reopen the stopped chat, and check retained history without restarting it.

Only the built-in demo agent runs. No real agent prompt, sign-in, installer or vendor configuration
mutation is performed. The native process restart and quit contracts remain covered by the separate
embedded smoke gate; this gate verifies the external desktop interaction boundary.

Each run writes a unique artifact directory containing a JSON verdict, a bounded driver log, and
PNG/HTML evidence after echo, stop and reopen (or failure when a session exists). The disposable
profile contents are not included in artifacts. Scratch folders are left to the runner's temporary
directory lifecycle. A four-minute watchdog ends the owned driver if the test hangs. Session deletion
is attempted first; termination of the still-owned tauri-driver closes its Windows kill-on-close Job
Object, covering EdgeDriver and the app even after failed session startup. It never kills by image
name or touches unrelated processes.

Run harness protocol, isolation and failure tests without launching any executable:

```text
node --test .github/scripts/webdriver-smoke.test.mjs
```

Passing these synthetic tests is not a Windows native acceptance result. The actual app gate must
pass on an ordinary Windows runner. macOS retains its separate native gates because this external
tauri-driver route has no WKWebView driver.

References: [Tauri manual WebDriver setup](https://v2.tauri.app/develop/tests/webdriver/manual-setup/),
[Microsoft WebView2 driver version requirement](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/webdriver),
and [pinned tauri-driver Windows job ownership](https://github.com/tauri-apps/tauri/blob/tauri-driver-v2.1.0/crates/tauri-driver/src/main.rs).
