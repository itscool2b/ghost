; Ghost's hooks for Tauri's NSIS uninstaller (tauri.conf.json: bundle > windows > nsis).
;
; What to delete is decided in Rust, by `ghost.exe --uninstall-cleanup`
; (apps/desktop/src-tauri/src/uninstall.rs, over ghost-core's uninstall module). This file only
; decides whether that runs, and stops Tauri's own "delete app data" step, which would remove the
; data folder whole, worktrees the person chose to keep included.
;
; - The confirm page's checkbox reads "Keep my Ghost data" (windows/English.nsh). Unticked,
;   which is the default, Ghost deletes its data. A quiet uninstall (/S or /P) deletes it too,
;   without dialogs, and keeps worktrees with unsaved work; /KEEPDATA keeps everything.
; - An update (/UPDATE) keeps everything, and so does the uninstall a newer installer runs before
;   installing: it runs this uninstaller in place (`_?=`), so $EXEDIR is the install folder, while
;   an uninstall started from Settings runs from a temporary copy.
; - The person's agents, their settings and their repositories are never touched.
;
; Exit codes of the cleanup: 0 done, 3 Ghost is still running (nothing deleted), 4 something
; could not be deleted (the cleanup listed it).

Var GhostDeleteData
Var GhostCleanupFlags
Var GhostCleanupLater

; Runs the cleanup from `program`; its exit code lands in $R9 ("none" when it did not start).
!macro GHOST_CLEANUP program
  ClearErrors
  ExecWait '"${program}" --uninstall-cleanup $GhostCleanupFlags' $R9
  ${If} ${Errors}
    StrCpy $R9 "none"
  ${EndIf}
!macroend

!macro GHOST_CLEANUP_RESULT
  ${If} $R9 == "0"
    DetailPrint "Ghost's data is deleted"
  ${ElseIf} $R9 == "4"
    DetailPrint "Some of Ghost's data could not be deleted"
  ${Else}
    DetailPrint "Ghost's data was not deleted (cleanup result $R9)"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $GhostDeleteData 1
  StrCpy $GhostCleanupLater 0
  ${If} $DeleteAppDataCheckboxState = 1
    StrCpy $GhostDeleteData 0
  ${EndIf}
  ; Tauri's own step removes both data folders whole; the cleanup does it around kept worktrees.
  StrCpy $DeleteAppDataCheckboxState 0
  ${If} $UpdateMode = 1
    StrCpy $GhostDeleteData 0
  ${EndIf}
  ${If} $EXEDIR == $INSTDIR
    StrCpy $GhostDeleteData 0
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/KEEPDATA" $R9
  ${IfNot} ${Errors}
    StrCpy $GhostDeleteData 0
  ${EndIf}
  ${If} $GhostDeleteData = 1
    DetailPrint "Deleting Ghost's data"
    StrCpy $GhostCleanupFlags "--interactive"
    ${If} $PassiveMode = 1
    ${OrIf} ${Silent}
      StrCpy $GhostCleanupFlags ""
    ${EndIf}
    !insertmacro GHOST_CLEANUP "$INSTDIR\${MAINBINARYNAME}.exe"
    ${If} $R9 == "3"
      ; Ghost did not quit when asked. Tauri's own check, next, offers to close it; the cleanup
      ; then runs after the files are removed, from a copy of the program.
      InitPluginsDir
      CopyFiles /SILENT "$INSTDIR\${MAINBINARYNAME}.exe" "$PLUGINSDIR\ghost-cleanup.exe"
      StrCpy $GhostCleanupLater 1
    ${Else}
      !insertmacro GHOST_CLEANUP_RESULT
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Earlier versions registered auracode:// links with Windows, for a sign-in page that no longer
  ; exists; this version's installer registers none, so its uninstaller would leave theirs behind.
  ; Remove the registration when it still points at this install, as their uninstallers did.
  ClearErrors
  ReadRegStr $R9 SHCTX "Software\Classes\auracode\shell\open\command" ""
  ${If} $R9 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
    DeleteRegKey SHCTX "Software\Classes\auracode"
  ${EndIf}
  ClearErrors
  ${If} $GhostCleanupLater = 1
    !insertmacro GHOST_CLEANUP "$PLUGINSDIR\ghost-cleanup.exe"
    !insertmacro GHOST_CLEANUP_RESULT
  ${EndIf}
  ${If} $GhostDeleteData = 1
    ; The installer's own records, as Tauri's "delete app data" step clears them.
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
    ; Windows' notification settings for Ghost.
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\${BUNDLEID}"
  ${EndIf}
!macroend

; Ghost was called o7 from 19 to 26 November 2026 and auracode before that, and a product's name
; is its install folder and its entry in Windows' list of apps. So an update from either installs
; Ghost beside it, and Tauri's template, in update mode, makes no shortcuts. This retires the
; earlier install once Ghost's files are in place: Ghost's shortcuts take the place of the earlier
; ones, a taskbar pin opens Ghost, and the earlier program, its entry in the list of apps and its
; installer records go.
;
; The earlier program is deleted only in its default per-user folder, only when its program file
; is in it, and never when Ghost was installed in or under that folder: a folder read from the
; registry is never deleted. An earlier copy installed elsewhere loses its entry in the list of
; apps and its shortcuts but keeps its files, so its uninstaller, which would delete the data
; Ghost now uses, is no longer offered. The data is never touched: Ghost opens it where it is
; (ghost-core's paths.rs). A start at sign-in turned on in the earlier copy is carried over by the
; app itself, the first time it starts (login_start.rs).
;
; OLDNAME is the earlier name: its product, its program and its publisher were one word. OLDID is
; its identifier.
!macro GHOST_RETIRE OLDNAME OLDID
  StrCpy $R7 "$LOCALAPPDATA\${OLDNAME}"
  ; Where the earlier entry in the list of apps says it is; the value is quoted.
  ClearErrors
  ReadRegStr $R5 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${OLDNAME}" "InstallLocation"
  StrCpy $R4 $R5 1
  ${If} $R4 == "$\""
    StrCpy $R5 $R5 "" 1
    StrCpy $R5 $R5 -1
  ${EndIf}
  ClearErrors
  ${If} ${FileExists} "$R7\${OLDNAME}.exe"
  ${OrIf} ${FileExists} "$R5\${OLDNAME}.exe"
    DetailPrint "Replacing ${OLDNAME} with Ghost"
    ; The Start menu and the desktop: Ghost's shortcut where the earlier one was.
    ${If} ${FileExists} "$SMPROGRAMS\${OLDNAME}.lnk"
      Delete "$SMPROGRAMS\${OLDNAME}.lnk"
    ${EndIf}
    ${IfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} ${FileExists} "$DESKTOP\${OLDNAME}.lnk"
      Delete "$DESKTOP\${OLDNAME}.lnk"
      ${IfNot} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
        CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
        !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
      ${EndIf}
    ${EndIf}
    ; A taskbar pin keeps its place and opens Ghost.
    StrCpy $R6 "$APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\${OLDNAME}.lnk"
    ${If} ${FileExists} $R6
      !insertmacro SetShortcutTarget $R6 "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId $R6
    ${EndIf}
    ; The earlier records: its entry in the list of apps, its install location, its notification
    ; settings, and a link registration from an early version still pointing at it.
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${OLDNAME}"
    DeleteRegKey HKCU "Software\${OLDNAME}\${OLDNAME}"
    DeleteRegKey /ifempty HKCU "Software\${OLDNAME}"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\${OLDID}"
    ClearErrors
    ReadRegStr $R6 HKCU "Software\Classes\${OLDNAME}\shell\open\command" ""
    ${If} $R6 == "$\"$R7\${OLDNAME}.exe$\" $\"%1$\""
    ${OrIf} $R6 == "$\"$R5\${OLDNAME}.exe$\" $\"%1$\""
      DeleteRegKey HKCU "Software\Classes\${OLDNAME}"
    ${EndIf}
    ClearErrors
    ; Its program, in its default folder only. The update closed it before this installer started.
    ${If} $LOCALAPPDATA != ""
    ${AndIf} ${FileExists} "$R7\${OLDNAME}.exe"
      GetFullPathName $R3 $R7
      GetFullPathName $R2 $INSTDIR
      StrLen $R1 "$R3\"
      StrCpy $R8 $R2 $R1
      ${If} $R2 != $R3
      ${AndIf} $R8 != "$R3\"
        RMDir /r $R3
        ${If} ${FileExists} "$R3\${OLDNAME}.exe"
          DetailPrint "The earlier program could not be removed yet: $R3"
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; The newest earlier name first: a machine that ran both has o7 installed and auracode retired.
!macro NSIS_HOOK_POSTINSTALL
  !insertmacro GHOST_RETIRE "o7" "codes.o7.desktop"
  !insertmacro GHOST_RETIRE "auracode" "si.auracode.desktop"
!macroend
