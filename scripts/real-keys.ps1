# Type into one window with real keyboard input, for as long as it stays focused.
#
# The synthetic typist inside the app puts characters into the raw input queue
# during a frame, so what it measures is the timer wake that produced the frame.
# A keystroke from a keyboard arrives as a window message instead, which can
# wake a loop the timer would not.  Telling the two apart needs input that came
# the same way a person's does, which is what SendInput is.
#
# Focus is checked before every character rather than taken: this types 'a'
# forever, and a window that quietly loses focus would otherwise be typing into
# whatever took it.
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
[StructLayout(LayoutKind.Explicit)] public struct INPUT {
  [FieldOffset(0)] public uint type;
  [FieldOffset(8)] public KEYBDINPUT ki;
}
'@

$KEYEVENTF_UNICODE = 0x0004
$KEYEVENTF_KEYUP = 0x0002

function New-UnicodeKey([char]$ch, [uint32]$extraFlags) {
  $input = New-Object Win.Input+INPUT
  $input.type = 1
  $ki = New-Object Win.Input+KEYBDINPUT
  $ki.wVk = 0
  $ki.wScan = [uint16]$ch
  $ki.dwFlags = $KEYEVENTF_UNICODE -bor $extraFlags
  $input.ki = $ki
  return $input
}

$down = New-UnicodeKey 'a' 0
$up = New-UnicodeKey 'a' $KEYEVENTF_KEYUP

$deadline = (Get-Date).AddSeconds($Seconds)
$sent = 0
$skipped = 0
while ((Get-Date) -lt $deadline) {
  $owner = 0
  [void][Win.Input]::GetWindowThreadProcessId([Win.Input]::GetForegroundWindow(), [ref]$owner)
  if ($owner -eq $TargetPid) {
    [void][Win.Input]::SendInput(2, @($down, $up), [System.Runtime.InteropServices.Marshal]::SizeOf([type]'Win.Input+INPUT'))
    $sent++
  } else {
    $skipped++
  }
  Start-Sleep -Milliseconds $EveryMs
}
Write-Output "sent $sent, skipped $skipped while unfocused"
