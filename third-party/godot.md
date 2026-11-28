# Godot

Ghost runs Godot projects in the Game pane with the Godot engine (<https://godotengine.org>, by
Juan Linietsky, Ariel Manzur and the Godot Engine contributors): it starts the game, shows its
frames, sends it input and reads its scene (`crates/ghost-game`, `docs/game.md`).

**Ghost does not ship Godot.** When a game needs it and the person presses Get Godot in the pane,
Ghost downloads one pinned release from Godot's own GitHub release page into its data folder
(`crates/ghost-game/src/godot.rs`). A Godot the person installed themselves is used instead when
it fits the project. No agent tool can start the download.

| | |
| --- | --- |
| Version | 4.7.2-stable |
| Release | https://github.com/godotengine/godot/releases/tag/4.7.2-stable |
| Licence | MIT (`LICENSE.txt` at the tag; third-party parts in its `COPYRIGHT.txt`) |
| Checksums | `SHA512-SUMS.txt` of that release |

| Platform | File | Size (bytes) | SHA-512 |
| --- | --- | --- | --- |
| Windows x86_64 | `Godot_v4.7.2-stable_win64.exe.zip` | 86,013,866 | `83decd58fdf67b9d657958a1ae6bf1929c20785315a81effe245874cdc57acb709bf868e00778a96984338c1b29dafdb453c6847747694621c6ecf5da2259993` |
| Windows arm64 | `Godot_v4.7.2-stable_windows_arm64.exe.zip` | 84,263,370 | `683f8dd9fb087db79dfbbc52d5b2209df98218a4fef0d10d8478ec2230ae8db6032a36929677479b9f9c5a6aa0c51ee359d7e57eb5abbcb6cef4998526dec5a6` |
| Linux x86_64 | `Godot_v4.7.2-stable_linux.x86_64.zip` | 77,860,424 | `9aa00f7a605200940bce3027a567b782f49bd8e940dd06ae9e987bd65aee1b1467edd56ed84fcdcbdd44354bf613bdbb4e5d2913e925850368e150c59ed54c65` |
| Linux arm64 | `Godot_v4.7.2-stable_linux.arm64.zip` | 77,008,699 | `dd59918da086bd49bde2f5450b5e567ff8650cbde9abbd7b8f4ca1197ff8c609baa38834666d032deafb47099078d7822279e2a0e06e5665745468f26533e7e2` |
| macOS universal | `Godot_v4.7.2-stable_macos.universal.zip` | 170,622,178 | `38aa16e5bba2083941fc5b3e54be0089bd4cc35e32415f5b9fd9a8a6a7b9818255d44532ea8ef94b5aef56c4b407c2d634fa4f657e4ebe681ebbf59b7bac69ca` |

The download is checked before anything in it is used: HTTPS only, redirects followed only to
GitHub's own hosts, the size and the SHA-512 above compared, and only then unpacked by
`ghost-fetch`, which lets no entry land outside its folder.

What Ghost itself adds to a running game is four GDScript files of its own
(`crates/ghost-game/resources/godot/`), loaded from Ghost's folder with `-s`. Nothing is written
into the person's project.

Moving to a later release means changing `VERSION` and the five pins in `godot.rs` and this record
together, from that release's `SHA512-SUMS.txt`, and running the **Godot game** workflow.
