; Tauri NSIS installer hooks.
;
; During an in-app update the running app exits and launches this installer, but
; the `airnote-backend.exe` sidecar can be left running (orphaned), keeping its
; .exe file locked. NSIS then aborts file replacement with "process is running".
; Force-close the app + sidecar before copying files so the update applies
; cleanly. taskkill ships with Windows, so no NSIS plugin is required.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /T /IM airnote-backend.exe'
  nsExec::Exec 'taskkill /F /IM AirNote.exe'
  ; Give the OS a moment to release the file handles before we copy.
  Sleep 1500
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /T /IM airnote-backend.exe'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
