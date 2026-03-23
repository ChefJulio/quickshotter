; QuickShotter - Explorer context menu registration
; Adds "Annotate with QuickShotter" to right-click menu for image files

!macro CUSTOM_INSTALL
  ; .png
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.png\shell\QuickShotter" "" "Annotate with QuickShotter"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.png\shell\QuickShotter" "Icon" "$INSTDIR\QuickShotter.exe"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.png\shell\QuickShotter\command" "" '"$INSTDIR\QuickShotter.exe" "--annotate" "%1"'

  ; .jpg
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\QuickShotter" "" "Annotate with QuickShotter"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\QuickShotter" "Icon" "$INSTDIR\QuickShotter.exe"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\QuickShotter\command" "" '"$INSTDIR\QuickShotter.exe" "--annotate" "%1"'

  ; .jpeg
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\QuickShotter" "" "Annotate with QuickShotter"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\QuickShotter" "Icon" "$INSTDIR\QuickShotter.exe"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\QuickShotter\command" "" '"$INSTDIR\QuickShotter.exe" "--annotate" "%1"'

  ; .webp
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.webp\shell\QuickShotter" "" "Annotate with QuickShotter"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.webp\shell\QuickShotter" "Icon" "$INSTDIR\QuickShotter.exe"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.webp\shell\QuickShotter\command" "" '"$INSTDIR\QuickShotter.exe" "--annotate" "%1"'

  ; .bmp
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\QuickShotter" "" "Annotate with QuickShotter"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\QuickShotter" "Icon" "$INSTDIR\QuickShotter.exe"
  WriteRegStr HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\QuickShotter\command" "" '"$INSTDIR\QuickShotter.exe" "--annotate" "%1"'
!macroend

!macro CUSTOM_UNINSTALL
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.png\shell\QuickShotter"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\QuickShotter"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\QuickShotter"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.webp\shell\QuickShotter"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\QuickShotter"
!macroend
