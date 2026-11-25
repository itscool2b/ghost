import O7Marks
import O7Theme
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
