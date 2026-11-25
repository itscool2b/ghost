import Foundation
import O7Client
import O7Store
import Testing

@Test func aComputerIsKeptWithItsKeyAndForgottenWithIt() throws {
    let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    let secrets = MemorySecretStore()
    let computers = Computers(folder: folder, secrets: secrets)
    let first = DeviceCredential(hostId: "host-a", deviceId: "dev-1", key: Data(repeating: 1, count: 32))
    #expect(try computers.paired(first, hostName: "Studio", site: "https://o7.codes", look: nil, features: ["inbox"]) == nil)
    #expect(computers.all().map(\.hostName) == ["Studio"])
    #expect(computers.credential(for: "host-a") == first)

    // Pairing the same computer again replaces the key, and hands back the device it replaced.
    let second = DeviceCredential(hostId: "host-a", deviceId: "dev-2", key: Data(repeating: 2, count: 32))
    let replaced = try computers.paired(second, hostName: "Studio", site: "https://o7.codes", look: nil, features: [])
    #expect(replaced == first)
    #expect(computers.all().count == 1)
    #expect(computers.credential(for: "host-a") == second)

    // Another computer is listed beside it, newest first.
    let other = DeviceCredential(hostId: "host-b", deviceId: "dev-3", key: Data(repeating: 3, count: 32))
    try computers.paired(other, hostName: "Laptop", site: "https://o7.codes", look: nil, features: [])
    #expect(computers.all().map(\.hostName) == ["Laptop", "Studio"])

    // A record whose key is gone is not listed: there is nothing to connect with.
    secrets.delete(account: "host-b")
    #expect(computers.all().map(\.hostName) == ["Studio"])

    computers.forget("host-a")
    #expect(computers.all().isEmpty)
    #expect(secrets.load(account: "host-a") == nil)
}
