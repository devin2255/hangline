; Hangline NSIS installer / uninstaller (P5-1).
;
; Per-user install (no UAC): default %LOCALAPPDATA%\Programs\Hangline.
; Compiled by scripts/release/build-release.mjs with the official NSIS 3.13 toolchain:
;
;   makensis /INPUTCHARSET UTF8 \
;     /DVERSION=<x.y.z> /DSOURCE_DIR=<staging dir> \
;     /DWEBVIEW2_SETUP=<MicrosoftEdgeWebView2Setup.exe> \
;     /DOUT_FILE=<release>\Hangline_<version>_x64-setup.exe \
;     scripts/release/installer.nsi
;
; SOURCE_DIR must contain Hangline.exe and Hangline.ico. VERSION must be a three-part
; numeric version (used by VIProductVersion). The source file is UTF-8; the /INPUTCHARSET
; UTF8 flag is required for the Simplified Chinese strings.
;
; Design constraints (spec §6.1 / §7 M17):
;  - never require administrator; registry writes are HKCU only
;  - embed the WebView2 bootstrapper and install it silently when the runtime is missing
;  - close a running Hangline before replacing files on upgrade
;  - uninstall keeps %LOCALAPPDATA%\Hangline by default; deletes it only on an explicit
;    "No" answer, HANG_UNINSTALL_DATA=delete, or /DELETEUSERDATA (silent uninstall defaults
;    to keep — never delete screenshots without an explicit choice)

Unicode true
ManifestDPIAware true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "Sections.nsh"
!include "StrFunc.nsh"

; UnStrLoc (uninstaller-side StrLoc) scans $CMDLINE for /DELETEUSERDATA.
${Using:StrFunc} UnStrLoc

; ---------------------------------------------------------------------------
; Build-time defines (injected by scripts/release/build-release.mjs)
; ---------------------------------------------------------------------------
!ifndef VERSION
  !error "VERSION is required: makensis /DVERSION=<x.y.z>"
!endif
!ifndef SOURCE_DIR
  !error "SOURCE_DIR is required: staging directory containing Hangline.exe and Hangline.ico"
!endif
!ifndef OUT_FILE
  !error "OUT_FILE is required: output path of the setup executable"
!endif
!ifndef WEBVIEW2_SETUP
  !error "WEBVIEW2_SETUP is required: path to MicrosoftEdgeWebView2Setup.exe"
!endif

!ifndef APP_NAME
  !define APP_NAME "Hangline"
!endif
!ifndef APP_EXE
  !define APP_EXE "Hangline.exe"
!endif
!ifndef APP_ICON
  !define APP_ICON "Hangline.ico"
!endif
!ifndef PUBLISHER
  !define PUBLISHER "Hangline"
!endif

!define APP_VERSION "${VERSION}"
!define UNINST_EXE "uninstall.exe"
; Registry keys are overridable so the P5-5 lifecycle test can install/uninstall into test-only
; HKCU keys without touching the real Add/Remove Programs entry or real Run/StartupApproved values.
!ifndef UNINSTALL_KEY
  !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!endif
; Installation paths are compile-time defines so a test-only build can relocate install dir,
; user data and shortcuts under %TEMP% (P5-5 lifecycle evidence) without touching real user data.
!ifndef APP_INSTALL_DIR
  !define APP_INSTALL_DIR "$LOCALAPPDATA\Programs\${APP_NAME}"
!endif
!ifndef APP_DATA_DIR
  !define APP_DATA_DIR "$LOCALAPPDATA\${APP_NAME}"
!endif
!ifndef SHORTCUT_DIR
  !define SHORTCUT_DIR "$SMPROGRAMS"
!endif
!ifndef DESKTOP_SHORTCUT_DIR
  !define DESKTOP_SHORTCUT_DIR "$DESKTOP"
!endif
!ifndef RUN_KEY
  !define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!endif
!ifndef STARTUP_APPROVED_KEY
  !define STARTUP_APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
!endif

; Official Microsoft Edge WebView2 Evergreen runtime client id (EdgeUpdate\Clients).
!define WEBVIEW2_CLIENT_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

; ---------------------------------------------------------------------------
; Installer metadata
; ---------------------------------------------------------------------------
Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "${APP_INSTALL_DIR}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64
CRCCheck on
BrandingText "${APP_NAME} ${APP_VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "ProductVersion" "${APP_VERSION}"
VIAddVersionKey "FileVersion" "${APP_VERSION}"
VIAddVersionKey "FileDescription" "${APP_NAME} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT License"

; ---------------------------------------------------------------------------
; Updater / passive-mode flags (Tauri updater invokes: /P /UPDATE /R /ARGS=<args>)
; ---------------------------------------------------------------------------
Var PassiveMode
Var UpdateMode
Var RestartMode
Var RestartArgs

; ---------------------------------------------------------------------------
; Modern UI 2 pages
; ---------------------------------------------------------------------------
!define MUI_ABORTWARNING
!define MUI_ICON "${SOURCE_DIR}\${APP_ICON}"
!define MUI_UNICON "${SOURCE_DIR}\${APP_ICON}"

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; ---------------------------------------------------------------------------
; Install
; ---------------------------------------------------------------------------
Section "$(SEC_MAIN_NAME)" SEC_MAIN
  SectionIn RO
  SetShellVarContext current

  ; Upgrade: stop the running app before overwriting the executable. First ask it to close
  ; gracefully, then force-kill as a fallback (the tray app ignores WM_CLOSE by design).
  Call CloseRunningApp

  ; WebView2: the embedded Evergreen bootstrapper downloads the runtime when it is missing
  ; (so a machine without WebView2 needs network during install). Abort before copying
  ; anything when the runtime is missing and cannot be installed. Update mode skips this:
  ; the runtime was already installed by the previous version.
  ${If} $UpdateMode != 1
    Call EnsureWebView2
  ${EndIf}

  SetOutPath "$INSTDIR"
  File "/oname=${APP_EXE}" "${SOURCE_DIR}\${APP_EXE}"
  File "/oname=${APP_ICON}" "${SOURCE_DIR}\${APP_ICON}"
  WriteUninstaller "$INSTDIR\${UNINST_EXE}"

  ; Start Menu shortcut (per-user). The optional desktop shortcut lives in its own section.
  CreateDirectory "$SMPROGRAMS"
  CreateShortCut "${SHORTCUT_DIR}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_ICON}" 0 SW_SHOWNORMAL "" "$(SHORTCUT_DESC)"

  ; Add/Remove Programs entry (HKCU, per-user).
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_ICON}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINST_EXE}"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINST_EXE}" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

  ; EstimatedSize is in KB; the WebView2 runtime is shared and not counted.
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0

  DetailPrint "Installed ${APP_NAME} ${APP_VERSION} to $INSTDIR"

  ; Tauri updater restart (/R) with optional /ARGS=<args>; the installer exits immediately
  ; after launching the new version so the update can complete without an extra prompt.
  ${If} $RestartMode == 1
    ${If} $RestartArgs != ""
      Exec '"$INSTDIR\${APP_EXE}" $RestartArgs'
    ${Else}
      Exec '"$INSTDIR\${APP_EXE}"'
    ${EndIf}
  ${EndIf}
SectionEnd

Section /o "$(SEC_DESKTOP_NAME)" SEC_DESKTOP
  SetShellVarContext current
  CreateShortCut "${DESKTOP_SHORTCUT_DIR}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_ICON}" 0 SW_SHOWNORMAL "" "$(SHORTCUT_DESC)"
SectionEnd

; Preselect the desktop shortcut on upgrade when the user already has one. Silent upgrades
; then preserve the previous shortcut choice instead of silently dropping it.
Function .onInit
  SetShellVarContext current

  ; Tauri updater: /P = passive, /UPDATE = update-in-place, /R = restart after install,
  ; /ARGS=<args> = arguments for the restarted app. Unknown flags are ignored.
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/R" $0
  ${IfNot} ${Errors}
    StrCpy $RestartMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/ARGS" $RestartArgs

  ${If} ${FileExists} "${DESKTOP_SHORTCUT_DIR}\${APP_NAME}.lnk"
    SectionSetFlags ${SEC_DESKTOP} ${SF_SELECTED}
  ${EndIf}
FunctionEnd

; Skips the wizard pages for passive/silent/update installs (progress page remains for /P).
Function SkipIfPassive
  ${If} ${Silent}
    Abort
  ${EndIf}
  ${If} $PassiveMode == 1
    Abort
  ${EndIf}
  ${If} $UpdateMode == 1
    Abort
  ${EndIf}
FunctionEnd

; ---------------------------------------------------------------------------
; Installer helper functions
; ---------------------------------------------------------------------------
Function CloseRunningApp
  DetailPrint "Closing running ${APP_NAME} instances (if any)..."
  ; Graceful first (taskkill without /F posts WM_CLOSE to top-level windows)...
  nsExec::Exec 'taskkill /IM "${APP_EXE}" /T'
  Pop $0
  Sleep 800
  ; ...then force, so the executable is never locked while being replaced.
  nsExec::Exec 'taskkill /IM "${APP_EXE}" /F /T'
  Pop $0
  Sleep 300
FunctionEnd

; Output: $0 = "1" when a WebView2 runtime registration exists, "" otherwise.
Function DetectWebView2
  StrCpy $0 ""
  SetRegView 64
  ReadRegStr $1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_CLIENT_GUID}" "pv"
  ${If} $1 != ""
    StrCpy $0 "1"
    Goto wv2_detect_done
  ${EndIf}
  ReadRegStr $1 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_CLIENT_GUID}" "pv"
  ${If} $1 != ""
    StrCpy $0 "1"
    Goto wv2_detect_done
  ${EndIf}
  SetRegView 32
  ReadRegStr $1 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_CLIENT_GUID}" "pv"
  ${If} $1 != ""
    StrCpy $0 "1"
    Goto wv2_detect_done
  ${EndIf}
  ReadRegStr $1 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_CLIENT_GUID}" "pv"
  ${If} $1 != ""
    StrCpy $0 "1"
  ${EndIf}
  wv2_detect_done:
  SetRegView 32
FunctionEnd

Function EnsureWebView2
  Call DetectWebView2
  ${If} $0 == "1"
    DetailPrint "Microsoft Edge WebView2 Runtime detected."
    Return
  ${EndIf}

  DetailPrint "$(WV2_INSTALLING)"
  InitPluginsDir
  File /oname=$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe "${WEBVIEW2_SETUP}"
  ExecWait '"$PLUGINSDIR\MicrosoftEdgeWebView2Setup.exe" /silent /install' $1
  DetailPrint "WebView2 bootstrapper exit code: $1"

  ; The bootstrapper normally registers the runtime before it exits; poll briefly in case
  ; EdgeUpdate writes the client key asynchronously.
  StrCpy $2 0
  ${Do}
    Call DetectWebView2
    ${If} $0 == "1"
      DetailPrint "Microsoft Edge WebView2 Runtime installed."
      Return
    ${EndIf}
    Sleep 1000
    IntOp $2 $2 + 1
  ${LoopUntil} $2 >= 30

  DetailPrint "ERROR: the WebView2 runtime is still missing after running the bootstrapper."
  MessageBox MB_ICONSTOP|MB_OK "$(WV2_FAIL)" /SD IDOK
  SetErrorLevel 3
  Quit
FunctionEnd

; ---------------------------------------------------------------------------
; Uninstall
; ---------------------------------------------------------------------------
Section "Uninstall"
  SetShellVarContext current

  ; M17: resolve keep/delete before removing anything. $R9 = "1" means delete user data.
  Call un.ResolveDataPolicy
  Call un.CloseRunningApp

  Delete "${SHORTCUT_DIR}\${APP_NAME}.lnk"
  Delete "${DESKTOP_SHORTCUT_DIR}\${APP_NAME}.lnk"

  ; Login item (autostart plugin, HKCU Run) and its Task Manager approval marker.
  DeleteRegValue HKCU "${RUN_KEY}" "${APP_NAME}"
  DeleteRegValue HKCU "${STARTUP_APPROVED_KEY}" "${APP_NAME}"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"

  ; Only ever delete the files this installer owns; unknown files in a user-chosen install
  ; directory are left in place (RMDir without /r removes the directory only when empty).
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_ICON}"
  Delete "$INSTDIR\${UNINST_EXE}"
  RMDir "$INSTDIR"

  ${If} $R9 == "1"
    ${If} $LOCALAPPDATA != ""
      RMDir /r "${APP_DATA_DIR}"
      DetailPrint "Deleted user data: ${APP_DATA_DIR}"
    ${EndIf}
  ${Else}
    DetailPrint "Kept user data: ${APP_DATA_DIR}"
  ${EndIf}
SectionEnd

Function un.CloseRunningApp
  DetailPrint "Closing running ${APP_NAME} instances (if any)..."
  nsExec::Exec 'taskkill /IM "${APP_EXE}" /T'
  Pop $0
  Sleep 800
  nsExec::Exec 'taskkill /IM "${APP_EXE}" /F /T'
  Pop $0
  Sleep 300
FunctionEnd

; Sets $R9 = "1" when the user explicitly chose to delete %LOCALAPPDATA%\Hangline.
; UI: Yes = keep (default), No = delete. Silent: keep unless HANG_UNINSTALL_DATA=delete or
; /DELETEUSERDATA is present. Closing the prompt without an answer also keeps data.
Function un.ResolveDataPolicy
  StrCpy $R9 ""
  ${If} ${Silent}
    ReadEnvStr $0 "HANG_UNINSTALL_DATA"
    ${If} $0 == "delete"
      StrCpy $R9 "1"
    ${EndIf}
    ${UnStrLoc} $0 "$CMDLINE" "/DELETEUSERDATA" ">"
    ${If} $0 != ""
      StrCpy $R9 "1"
    ${EndIf}
  ${Else}
    MessageBox MB_YESNO|MB_ICONQUESTION "$(UN_DATA_PROMPT)" /SD IDYES IDYES un_keep_data IDNO un_delete_data
    StrCpy $R9 ""
    Goto un_policy_done
    un_delete_data:
      StrCpy $R9 "1"
      Goto un_policy_done
    un_keep_data:
      StrCpy $R9 ""
    un_policy_done:
  ${EndIf}
FunctionEnd

; ---------------------------------------------------------------------------
; Component descriptions
; ---------------------------------------------------------------------------
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN} "$(SEC_MAIN_DESC)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "$(SEC_DESKTOP_DESC)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------------------------------------------------------------------
; Languages (installer UI follows the Windows user language)
; ---------------------------------------------------------------------------
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"

LangString SEC_MAIN_NAME ${LANG_ENGLISH} "Hangline (required)"
LangString SEC_MAIN_NAME ${LANG_SIMPCHINESE} "Hangline（必需）"
LangString SEC_MAIN_DESC ${LANG_ENGLISH} "Hangline application files and the Start Menu shortcut."
LangString SEC_MAIN_DESC ${LANG_SIMPCHINESE} "Hangline 程序文件与开始菜单快捷方式。"

LangString SEC_DESKTOP_NAME ${LANG_ENGLISH} "Desktop shortcut"
LangString SEC_DESKTOP_NAME ${LANG_SIMPCHINESE} "桌面快捷方式"
LangString SEC_DESKTOP_DESC ${LANG_ENGLISH} "Create a Hangline shortcut on the desktop (optional)."
LangString SEC_DESKTOP_DESC ${LANG_SIMPCHINESE} "在桌面创建 Hangline 快捷方式（可选）。"

LangString SHORTCUT_DESC ${LANG_ENGLISH} "Hangline screenshot tool"
LangString SHORTCUT_DESC ${LANG_SIMPCHINESE} "Hangline 截图工具"

LangString WV2_INSTALLING ${LANG_ENGLISH} "Microsoft Edge WebView2 Runtime is missing; installing it silently (this can take a minute)..."
LangString WV2_INSTALLING ${LANG_SIMPCHINESE} "未检测到 Microsoft Edge WebView2 运行时，正在静默安装（可能需要一分钟）……"

LangString WV2_FAIL ${LANG_ENGLISH} "Hangline requires the Microsoft Edge WebView2 Runtime, but it could not be installed.$\r$\n$\r$\nPlease install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703 and run this installer again.$\r$\n$\r$\nSetup will now exit."
LangString WV2_FAIL ${LANG_SIMPCHINESE} "Hangline 需要 Microsoft Edge WebView2 运行时，但自动安装失败。$\r$\n$\r$\n请从 https://go.microsoft.com/fwlink/p/?LinkId=2124703 手动安装后重新运行本安装程序。$\r$\n$\r$\n安装程序即将退出。"

LangString UN_DATA_PROMPT ${LANG_ENGLISH} "Keep your Hangline data?$\r$\n$\r$\nYes (recommended): keep settings, thumbnails and screenshots in $LOCALAPPDATA\Hangline.$\r$\nNo: permanently delete that folder, including all screenshots and settings.$\r$\n$\r$\nThe application itself will be uninstalled either way."
LangString UN_DATA_PROMPT ${LANG_SIMPCHINESE} "是否保留 Hangline 数据？$\r$\n$\r$\n是（推荐）：保留 $LOCALAPPDATA\Hangline 中的设置、缩略图和截图。$\r$\n否：永久删除该文件夹（包含所有截图和设置）。$\r$\n$\r$\n无论选择哪一项，应用程序都会被卸载。"
