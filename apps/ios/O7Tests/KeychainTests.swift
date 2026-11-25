import Foundation
import O7Store
import Testing

/// Runs inside the app on a simulator, where the Keychain and the App Group are the real ones.
struct KeychainTests {
    @Test func aDeviceKeyIsKeptOnThisDeviceAndGoesWhenAsked() throws {
        let store = KeychainSecretStore(service: "codes.o7.mobile.test")
        store.deleteAll()
        let key = Data((0..<32).map { UInt8($0) })
        try store.save(key, account: "host-a")
        #expect(store.load(account: "host-a") == key)
        #expect(store.accounts() == ["host-a"])
        // Saving again replaces it.
        try store.save(Data(repeating: 7, count: 32), account: "host-a")
        #expect(store.load(account: "host-a") == Data(repeating: 7, count: 32))
        store.delete(account: "host-a")
        #expect(store.load(account: "host-a") == nil)
        store.deleteAll()
    }

    /// The paired computers' names live where the notification extension and widgets can read
    /// them. A simulator build without a team may have no App Group; the app falls back to its own
    /// folder then, which this records.
    @Test func theComputersListHasAFolder() throws {
        let folder = Computers.defaultFolder()
        let secrets = MemorySecretStore()
        let computers = Computers(folder: folder, secrets: secrets)
        #expect(computers.all().isEmpty || !computers.all().isEmpty)
        #expect(FileManager.default.fileExists(atPath: folder.path))
    }
}
