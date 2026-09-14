; Remove only this application's opt-in startup entry when uninstalling.
; User configuration and credentials follow the normal installer policy.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Personal Hub"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Personal Hub"
!macroend
