!macro NSIS_HOOK_POSTUNINSTALL
  ; Preserve profiles and settings during in-place updates, but make a real
  ; uninstall a clean reset. Perfect-Sync intentionally stores its authoritative
  ; data under the product name rather than Tauri's bundle identifier.
  ${If} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\Perfect-Sync"
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\Perfect-Sync"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"

    ; Remove the GitHub token stored by keyring 2.x as
    ; <username>.<service> in Windows Credential Manager.
    nsExec::ExecToLog '"$SYSDIR\cmdkey.exe" /delete:github-token.com.artriy.perfectsync'
    Pop $0

    ; The stock uninstaller removes this protocol only when its command matches
    ; exactly. Remove the app-owned key even after a broken or moved install.
    DeleteRegKey HKCU "Software\Classes\perfectsync"

    ; Remove Windows compatibility records that point at this installation.
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store" "$INSTDIR\${MAINBINARYNAME}.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store" "$INSTDIR\Perfect-Sync.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store" "$INSTDIR\uninstall.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Persisted" "$INSTDIR\${MAINBINARYNAME}.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Persisted" "$INSTDIR\Perfect-Sync.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Persisted" "$INSTDIR\uninstall.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers" "$INSTDIR\${MAINBINARYNAME}.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers" "$INSTDIR\Perfect-Sync.exe"
    DeleteRegValue HKCU "Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers" "$INSTDIR\uninstall.exe"
  ${EndIf}
!macroend
