; Remove only this application's opt-in startup entry when uninstalling.
; User configuration and credentials follow the normal installer policy.
; "Personal Hub" is the name builds before the Atrium rename registered under;
; an uninstall must clear it too, or Windows keeps trying to launch a path that
; no longer exists at every sign-in.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Atrium"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Atrium"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Personal Hub"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Personal Hub"
!macroend
