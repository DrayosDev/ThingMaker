Option Explicit

If WScript.Arguments.Count <> 2 Then
    WScript.Quit 2
End If

Dim files, cloudflared, config
Set files = CreateObject("Scripting.FileSystemObject")
cloudflared = WScript.Arguments(0)
config = WScript.Arguments(1)
If Not files.FileExists(cloudflared) Or Not files.FileExists(config) Then
    WScript.Quit 2
End If

Dim shell, command
Set shell = CreateObject("WScript.Shell")
command = Chr(34) & cloudflared & Chr(34) & " tunnel --config " & Chr(34) & config & Chr(34) & " run"

Do
    ' Wait for cloudflared to exit before starting a replacement.
    On Error Resume Next
    shell.Run command, 0, True
    On Error GoTo 0
    WScript.Sleep 5000
Loop
