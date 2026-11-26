/**
 * The Windows uninstaller's contract with Tauri's NSIS template, checked without building an
 * installer: the hooks rely on the pinned template's variables and order, and the custom English
 * file must define every string the template uses. Reads the pinned CLI's embedded template when
 * its Windows binary is installed (it is only on Windows).
 */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauri = resolve(app, "src-tauri");
const config = JSON.parse(readFileSync(resolve(tauri, "tauri.conf.json"), "utf8"));
const nsis = config.bundle.windows.nsis;
const hooks = readFileSync(resolve(tauri, nsis.installerHooks), "utf8");
const english = readFileSync(resolve(tauri, nsis.customLanguageFiles.English), "utf8");

/** The pinned Tauri CLI's Windows binary, which embeds the installer template and its strings. */
function cliBinary() {
  try {
    const cli = createRequire(resolve(app, "package.json")).resolve("@tauri-apps/cli/package.json");
    const native = createRequire(cli).resolve("@tauri-apps/cli-win32-x64-msvc/package.json");
    const binary = resolve(dirname(native), "cli.win32-x64-msvc.node");
    return existsSync(binary) ? readFileSync(binary) : null;
  } catch {
    return null;
  }
}
const binary = process.platform === "win32" ? cliBinary() : null;

/** The installer template embedded in the CLI: from its first line to its last function. */
function template(bytes) {
  const uninstall = bytes.indexOf("Section Uninstall");
  const start = bytes.lastIndexOf("Unicode true", uninstall);
  return bytes.subarray(start, bytes.indexOf(0, start)).toString("utf8");
}

const keys = (text) =>
  [...text.matchAll(/^LangString (\w+) \$\{LANG_ENGLISH\}/gmu)].map((m) => m[1]);

const has = (text, fragment, message) => assert.ok(text.includes(fragment), message ?? fragment);
/** NSIS spellings that read as template placeholders or escapes in JavaScript. */
const BIN = `$${"{"}MAINBINARYNAME}`;
const SEP = String.fromCharCode(92);

test("the bundle builds only the NSIS installer on Windows, with the hooks and strings", () => {
  // Windows builds NSIS only: no MSI without the cleanup (the Linux packages sit beside them).
  assert.ok(config.bundle.targets.includes("nsis"), "the NSIS installer");
  assert.ok(!config.bundle.targets.includes("msi"), "no MSI without the cleanup");
  assert.deepEqual(nsis.languages, ["English"]);
  assert.match(hooks, /^!macro NSIS_HOOK_PREUNINSTALL$/mu);
  assert.match(hooks, /^!macro NSIS_HOOK_POSTUNINSTALL$/mu);
  const dash = String.fromCharCode(0x2014);
  assert.ok(!hooks.includes(dash) && !english.includes(dash), "no em dashes");
});

test("the hooks run the Rust cleanup and keep Tauri from deleting the data folder whole", () => {
  has(hooks, `ExecWait '"\${program}" --uninstall-cleanup $GhostCleanupFlags' $R9`);
  has(hooks, `!insertmacro GHOST_CLEANUP "$INSTDIR${SEP}${BIN}.exe"`);
  has(hooks, "StrCpy $DeleteAppDataCheckboxState 0");
  // Updates and the uninstall an installer runs before installing keep the data.
  has(hooks, `\${If} $UpdateMode = 1\n    StrCpy $GhostDeleteData 0`);
  has(hooks, `\${If} $EXEDIR == $INSTDIR\n    StrCpy $GhostDeleteData 0`);
  has(hooks, `\${GetOptions} $CMDLINE "/KEEPDATA" $R9`);
  // A quiet uninstall never shows a dialog.
  has(hooks, `\${OrIf} \${Silent}\n      StrCpy $GhostCleanupFlags ""`);
  // Still running: Tauri's own check closes Ghost, then a copy of the program cleans up. Its
  // check is inserted once per section, since its labels come from the line number.
  assert.ok(!hooks.includes("!insertmacro CheckIfAppIsRunning"));
  has(hooks, `CopyFiles /SILENT "$INSTDIR${SEP}${BIN}.exe" "$PLUGINSDIR${SEP}ghost-cleanup.exe"`);
  has(
    hooks,
    `\${If} $GhostCleanupLater = 1\n    !insertmacro GHOST_CLEANUP "$PLUGINSDIR${SEP}ghost-cleanup.exe"`,
  );
  // The exit codes the hooks read are the Rust ones.
  const rust = readFileSync(resolve(tauri, "src/uninstall.rs"), "utf8");
  has(rust, "pub const RUNNING: i32 = 3;");
  has(rust, "pub const INCOMPLETE: i32 = 4;");
  has(hooks, `\${If} $R9 == "3"`);
  has(hooks, `\${ElseIf} $R9 == "4"`);
});

test("a link registration an earlier version left is removed only when it is this install's", () => {
  // tauri.conf.json registers no scheme any more, so Tauri's template no longer removes one.
  assert.equal(config.plugins?.["deep-link"], undefined);
  const key = `Software${SEP}Classes${SEP}auracode`;
  has(hooks, `ReadRegStr $R9 SHCTX "${key}${SEP}shell${SEP}open${SEP}command" ""`);
  has(
    hooks,
    `\${If} $R9 == "$${SEP}"$INSTDIR${SEP}${BIN}.exe$${SEP}" $${SEP}"%1$${SEP}""\n    DeleteRegKey SHCTX "${key}"`,
  );
  // It runs before the cleanup reads its own result from the same register.
  assert.ok(hooks.indexOf(`DeleteRegKey SHCTX "${key}"`) < hooks.indexOf("$GhostCleanupLater = 1"));
});

test("the checkbox keeps the data, and every other string is Tauri's", () => {
  has(english, `LangString deleteAppData \${LANG_ENGLISH} "Keep my Ghost data`);
  assert.ok(keys(english).length >= 20);
});

test("the pinned template still inserts the hooks where they rely on it", { skip: !binary }, () => {
  const text = template(binary);
  assert.match(text, /!include "\{\{installer_hooks\}\}"/u);
  assert.match(text, /^Var DeleteAppDataCheckboxState$/mu);
  assert.match(text, /^Var UpdateMode$/mu);
  assert.match(text, /^Var PassiveMode$/mu);
  const section = text.slice(text.indexOf("Section Uninstall"));
  const hook = section.indexOf("!insertmacro NSIS_HOOK_PREUNINSTALL");
  const running = section.indexOf("!insertmacro CheckIfAppIsRunning");
  const removal = section.indexOf(`\${If} $DeleteAppDataCheckboxState = 1`);
  const post = section.indexOf("!insertmacro NSIS_HOOK_POSTUNINSTALL");
  assert.ok(hook >= 0 && hook < running, "the cleanup runs before the app check and file removal");
  assert.ok(running < removal && removal < post, "Tauri's data step reads the reset checkbox");
  assert.match(section.slice(removal, post), /RmDir \/r "\$LOCALAPPDATA\\\$\{BUNDLEID\}"/u);
  assert.match(section.slice(removal, post), /DeleteRegKey SHCTX "\$\{MANUPRODUCTKEY\}"/u);
  // The checkbox is created unticked: ours reads "Keep", so deleting is the default.
  assert.doesNotMatch(text, /DeleteAppDataCheckbox \$\{BM_SETCHECK\}/u);
});

test("the custom English file defines every string the pinned CLI's does", {
  skip: !binary,
}, () => {
  const text = binary.toString("utf8");
  const start = text.indexOf(`LangString addOrReinstall \${LANG_ENGLISH}`);
  const end = text.indexOf("\n", text.indexOf(`LangString deleteAppData \${LANG_ENGLISH}`, start));
  assert.deepEqual(keys(english).sort(), keys(text.slice(start, end)).sort());
});

test("an update from o7 or from auracode retires that install, and only that", () => {
  const start = hooks.indexOf("!macro GHOST_RETIRE OLDNAME OLDID");
  assert.notEqual(start, -1, "the retire step is one macro, used for each earlier name");
  const retire = hooks.slice(start, hooks.indexOf("!macroend", start));
  // It runs after the install, from the hook Tauri's template inserts there: once for each
  // earlier name, the newest first.
  has(
    hooks,
    '!macro NSIS_HOOK_POSTINSTALL\n  !insertmacro GHOST_RETIRE "o7" "codes.o7.desktop"\n  !insertmacro GHOST_RETIRE "auracode" "si.auracode.desktop"\n!macroend',
  );
  // Only the default per-user folder, only with the earlier program in it, never Ghost's own.
  has(retire, `StrCpy $R7 "$LOCALAPPDATA${SEP}\${OLDNAME}"`);
  assert.equal([...retire.matchAll(/RMDir/g)].length, 1, "one folder is deleted");
  has(retire, "RMDir /r $R3\n");
  // What is deleted is the default folder, made full, and only with the earlier program in it,
  // LOCALAPPDATA known, and Ghost neither in it nor under it.
  const guard = retire.slice(retire.indexOf("${If} $LOCALAPPDATA"), retire.indexOf("RMDir"));
  has(guard, `\${If} $LOCALAPPDATA != ""\n    \${AndIf} \${FileExists} "$R7${SEP}\${OLDNAME}.exe"`);
  has(guard, "GetFullPathName $R3 $R7\n      GetFullPathName $R2 $INSTDIR");
  has(guard, `\${If} $R2 != $R3\n      \${AndIf} $R8 != "$R3${SEP}"`);
  assert.doesNotMatch(retire, /ReadRegStr \$R[37]/, "the folder never comes from the registry");
  assert.doesNotMatch(retire, /RMDir[^\n]*\$R5/, "an install found elsewhere keeps its files");
  // Shortcuts: Ghost's in place of the earlier ones; a taskbar pin is retargeted, not removed.
  has(
    retire,
    `CreateShortcut "$SMPROGRAMS${SEP}\${PRODUCTNAME}.lnk" "$INSTDIR${SEP}\${MAINBINARYNAME}.exe"`,
  );
  has(retire, `!insertmacro SetShortcutTarget $R6 "$INSTDIR${SEP}\${MAINBINARYNAME}.exe"`);
  // The earlier records go; its data is never named.
  has(
    retire,
    `DeleteRegKey HKCU "Software${SEP}Microsoft${SEP}Windows${SEP}CurrentVersion${SEP}Uninstall${SEP}\${OLDNAME}"`,
  );
  for (const line of retire.split("\n").filter((text) => /^\s*(Delete|RMDir)\b/.test(text)))
    assert.doesNotMatch(
      line,
      /OLDID|si\.auracode|codes\.o7/,
      `an earlier data folder is not touched: ${line}`,
    );
  assert.doesNotMatch(
    retire,
    /uninstall\.exe/,
    "the earlier uninstaller, which deletes data, never runs",
  );
});
