import O7Marks
import O7Push
import O7Theme
import O7Wire
import SwiftUI

/// The paired computers, how the app looks, what it guards with Face ID, and about.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    @State private var forgetting: ComputerStore?

    var body: some View {
        @Bindable var settings = model.settings
        NavigationStack {
            Form {
                Section("Computers") {
                    ForEach(model.computers) { computer in
                        HStack(spacing: 12) {
                            Image(systemName: "desktopcomputer").foregroundStyle(style.fg2)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(computer.hostName).o7Font(.rowTitle)
                                ConnectionLine(phase: computer.phase)
                            }
                            Spacer()
                            // The computer's theme, as the app wears it while that computer is in view.
                            let swatch = Palettes.palette(
                                ThemeID(rawValue: computer.look.theme) ?? .default, dark: computer.look.appearance != "light")
                            Circle()
                                .fill(Color(swatch.primary))
                                .overlay(Circle().strokeBorder(Color(swatch.window), lineWidth: 3))
                                .frame(width: 22, height: 22)
                                .accessibilityLabel("Theme \(ThemeID(rawValue: computer.look.theme)?.name ?? computer.look.theme)")
                        }
                        .swipeActions {
                            Button("Forget", role: .destructive) { forgetting = computer }
                        }
                    }
                    Button("Pair another computer", systemImage: "qrcode.viewfinder") {
                        dismiss()
                        model.scanning = true
                    }
                }
                Section {
                    Picker("Theme", selection: Binding(get: { settings.themeOverride ?? "" }, set: { settings.themeOverride = $0.isEmpty ? nil : $0 })) {
                        Text("From the computer").tag("")
                        ForEach(ThemeID.allCases, id: \.rawValue) { theme in
                            Text(theme.name).tag(theme.rawValue)
                        }
                    }
                    Picker("Appearance", selection: Binding(get: { settings.appearance ?? "" }, set: { settings.appearance = $0.isEmpty ? nil : $0 })) {
                        Text("From the computer").tag("")
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                } header: {
                    Text("Look")
                } footer: {
                    Text("o7 wears the theme of the computer in view, as you set it in o7's Settings there.")
                }
                NotificationsSection()
                Section {
                    Toggle("Face ID to allow what agents ask", isOn: $settings.faceIdToApprove)
                    Toggle("Lock o7 with Face ID", isOn: $settings.lockWithFaceId)
                } header: {
                    Text("Security")
                }
                Section("About") {
                    LabeledContent("Version", value: ThisDevice.appVersion)
                    Link("Terms of Service", destination: URL(string: "https://o7.codes/terms")!)
                    Link("Privacy Policy", destination: URL(string: "https://o7.codes/privacy")!)
                    NavigationLink("Acknowledgements") { Acknowledgements() }
                    Text("o7 is a product of BASH.AI LLC.").o7Font(.meta).foregroundStyle(style.fg3)
                }
                #if O7_DEV_TOOLS
                    DeveloperSection()
                #endif
            }
            .o7Font(.ui(16))
            .scrollContentBackground(.hidden)
            .background(style.pane.ignoresSafeArea())
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .confirmationDialog(
                "Forget \(forgetting?.hostName ?? "this computer")?",
                isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } }),
                titleVisibility: .visible
            ) {
                Button("Forget", role: .destructive) {
                    if let computer = forgetting { Task { await model.forget(computer) } }
                }
            } message: {
                Text("This iPhone's key is deleted here, and the computer stops accepting it. Pair again from o7's Settings to come back.")
            }
        }
    }
}

/// Notifications: on or off for the app, what each computer sends, and a test.
struct NotificationsSection: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.openURL) private var openURL

    var body: some View {
        @Bindable var push = model.push
        Section {
            switch push.status {
            case .notDetermined:
                Button("Turn on notifications", systemImage: "bell.badge") {
                    Task { await model.push.turnOn() }
                }
            case .denied:
                Button("Allow notifications in Settings", systemImage: "gear") {
                    if let url = URL(string: UIApplication.openNotificationSettingsURLString) { openURL(url) }
                }
            default:
                ForEach(model.computers) { computer in
                    NavigationLink {
                        ComputerNotifications(computer: computer)
                    } label: {
                        LabeledContent(computer.hostName, value: words(computer))
                    }
                }
                Toggle("Live Activity while agents work", isOn: $push.liveActivities)
            }
        } header: {
            Text("Notifications")
        } footer: {
            Text(footer)
        }
    }

    private func words(_ computer: ComputerStore) -> String {
        if !computer.offersPush { return "Update o7 there" }
        if computer.relayPushes == false { return "Relay cannot send" }
        let prefs = model.push.prefs(computer.hostId)
        return [prefs.asking, prefs.finished, prefs.failed, prefs.tasks].contains(true) ? "On" : "Off"
    }

    private var footer: String {
        switch model.push.status {
        case .denied: "Notifications are off for o7 in this iPhone's Settings."
        case .notDetermined:
            "Know when an agent needs you, finishes or fails. What a notification says is sealed on your computer for this iPhone: the relay and Apple carry it without reading it."
        default:
            model.push.failure.map { "This iPhone could not register for notifications: \($0)" }
                ?? "Sealed on your computer for this iPhone: the relay and Apple carry them without reading them."
        }
    }
}

/// What one computer sends this phone.
struct ComputerNotifications: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let computer: ComputerStore
    @State private var prefs = PushPrefs.all
    @State private var tested: String?

    var body: some View {
        Form {
            if !computer.offersPush {
                Text("The o7 on \(computer.hostName) does not send notifications yet. Update o7 there.")
                    .foregroundStyle(style.fg3)
            } else {
                if computer.relayPushes == false {
                    Section {
                        Text("The relay \(computer.hostName) uses does not send notifications yet.")
                            .foregroundStyle(style.amber)
                    }
                }
                Section {
                    Toggle("Needs you", isOn: $prefs.asking)
                    Toggle("Finished", isOn: $prefs.finished)
                    Toggle("Failed", isOn: $prefs.failed)
                    Toggle("Tasks", isOn: $prefs.tasks)
                } footer: {
                    Text("Tasks: a review is in, or work queued for tonight could not start.")
                }
                Section {
                    Picker("Chats", selection: $prefs.scope) {
                        Text("Every chat").tag("all")
                        Text("Chats from this iPhone").tag("mine")
                    }
                } footer: {
                    Text("Chats from this iPhone: those you started, sent to or answered here.")
                }
                Section {
                    Button("Send a test notification", systemImage: "paperplane") {
                        Task {
                            do {
                                try await computer.connection.perform(.pushTest)
                                tested = "Sent. It arrives in a few seconds."
                            } catch {
                                tested = (error as? LocalizedError)?.errorDescription
                            }
                        }
                    }
                } footer: {
                    if let tested { Text(tested) }
                }
            }
        }
        .o7Font(.ui(16))
        .scrollContentBackground(.hidden)
        .background(style.pane.ignoresSafeArea())
        .navigationTitle(computer.hostName)
        .onAppear { prefs = model.push.prefs(computer.hostId) }
        .onChange(of: prefs) { _, next in
            model.push.setPrefs(next, for: computer.hostId)
            Task { await computer.registerPush() }
        }
    }
}

/// What the app is built with, and their licences.
struct Acknowledgements: View {
    @Environment(\.style) private var style
    var body: some View {
        List {
            Section {
                Text("Geist and Geist Mono, by Vercel. SIL Open Font License 1.1.")
                Text("highlight.js, by Ivan Sagalaev and others. BSD 3-Clause License.")
            } footer: {
                Text("Agent logos belong to their makers and are used to name their agents.")
            }
        }
        .navigationTitle("Acknowledgements")
    }
}

#if O7_DEV_TOOLS
    /// A relay of the developer's own, for every computer (development builds only).
    struct DeveloperSection: View {
        @AppStorage("O7Relay") private var relay = ""
        var body: some View {
            Section {
                TextField("ws://127.0.0.1:8799", text: $relay)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            } header: {
                Text("Developer")
            } footer: {
                Text("A relay for every computer, read when the app starts. Leave empty for each computer's own site.")
            }
        }
    }
#endif
