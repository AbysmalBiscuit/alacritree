# Type into one window with real keyboard input, for as long as it stays focused.
#
# The synthetic typist inside the app puts characters into the raw input queue
# during a frame, so what it measures is the timer wake that produced the frame.
# A keystroke from a keyboard arrives as a window message instead, which can
# wake a loop the timer would not.  Telling the two apart needs input that came
# the same way a person's does, which is what SendInput is.
#
# Focus is checked before every character rather than taken: this types 'a' for
# minutes at a time, and a window that quietly loses focus would otherwise be
# typing into whatever took it.  The count is reported as it goes, because a run
# that is typing into nothing has to say so while there is still time to fix it
# rather than at the end.
param(
  [Parameter(Mandatory = $true)][int]$TargetPid,
  [Parameter(Mandatory = $true)][int]$Seconds,
  [int]$EveryMs = 60
)

Add-Type -Namespace Win -Name Input -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
[DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
[StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
  public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
}
// Size is stated rather than inferred: only the keyboard arm of the union is
// declared here, and without it the struct measures 32 bytes where the real
// INPUT is 40 — the mouse arm is the larger one.  SendInput checks the size it
// is handed against its own and refuses every call that disagrees.
[StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT {
  [FieldOffset(0)] public uint type;
  [FieldOffset(8)] public KEYBDINPUT ki;
}
'@

if ([IntPtr]::Size -ne 8) { throw "needs 64-bit PowerShell; INPUT is 40 bytes only on x64" }

$KEYEVENTF_UNICODE = 0x0004
$KEYEVENTF_KEYUP = 0x0002
$INPUT_KEYBOARD = 1

function New-UnicodeKey([char]$ch, [uint32]$extraFlags) {
  $evt = New-Object Win.Input+INPUT
  $evt.type = $INPUT_KEYBOARD
  $ki = New-Object Win.Input+KEYBDINPUT
  $ki.wVk = 0
  $ki.wScan = [uint16]$ch
  $ki.dwFlags = $KEYEVENTF_UNICODE -bor $extraFlags
  $evt.ki = $ki
  return $evt
}

$down = New-UnicodeKey 'a' 0
$up = New-UnicodeKey 'a' $KEYEVENTF_KEYUP
$size = [System.Runtime.InteropServices.Marshal]::SizeOf([type]'Win.Input+INPUT')

# One activation, not a fight: the window has to be foreground for SendInput
# to reach it, but re-taking focus every few seconds from a machine someone is
# using would be worse than a lost run.
(New-Object -ComObject WScript.Shell).AppActivate($TargetPid) | Out-Null
Start-Sleep -Milliseconds 500

$deadline = (Get-Date).AddSeconds($Seconds)
$nextReport = (Get-Date).AddSeconds(5)
$sent = 0
$skipped = 0
$failed = 0
while ((Get-Date) -lt $deadline) {
  $owner = 0
  [void][Win.Input]::GetWindowThreadProcessId([Win.Input]::GetForegroundWindow(), [ref]$owner)
  if ($owner -eq $TargetPid) {
    if ([Win.Input]::SendInput(2, @($down, $up), $size) -eq 2) { $sent++ } else { $failed++ }
  } else {
    $skipped++
  }
  if ((Get-Date) -ge $nextReport) {
    Write-Output "typist: sent $sent, skipped $skipped unfocused, $failed rejected"
    $nextReport = (Get-Date).AddSeconds(5)
  }
  Start-Sleep -Milliseconds $EveryMs
}
Write-Output "typist done: sent $sent, skipped $skipped unfocused, $failed rejected"
