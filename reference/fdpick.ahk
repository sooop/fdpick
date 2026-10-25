#Requires AutoHotkey v2.0
#SingleInstance Force

; Ctrl+Alt+Space : fdpick 실행 (이미 떠 있으면 그 창으로 포커스)
^!Space:: LaunchPicker()

LaunchPicker() {
    crit := "fdpick ahk_class CASCADIA_HOSTING_WINDOW_CLASS"
    if WinExist(crit) {
        WinActivate(crit)
        return
    }
    script := A_ScriptDir "\fdpick.ps1"
    Run('wt.exe -w new new-tab --title fdpick --suppressApplicationTitle'
        . ' pwsh.exe -NoProfile -File "' script '"')
    if !WinWait(crit, , 5)
        return
    w := 1100, h := 640
    WinMove((A_ScreenWidth - w) // 2, (A_ScreenHeight - h) // 3, w, h, crit)
    WinActivate(crit)
}
