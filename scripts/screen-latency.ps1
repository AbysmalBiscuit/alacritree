# Time a burst of keystrokes from SendInput to the pixels settling on screen.
#
# Every measurement taken inside the app stops early.  `echo` ends when the PTY
# thread sees the byte, the frame log counts calls to `update`, and the GPU
# timer covers the draw call - none include tessellation, the driver's queue,
# or DWM composing the frame.  An app whose frames are piling up reads as
# healthy from inside while the screen runs seconds behind.
#
# What is hashed is the window's own DWM surface, which holds the frame most
# recently presented, so a frame still queued in the driver reads as the old
# one.  That also survives the window being covered, which under a tiling
# window manager it regularly is.
#
#   screen-latency.ps1 -TargetPid 1234 -Samples 20 -Burst 15
#
# Bursts rather than single keys, because the reported failure is about typing
# fast: nothing appears, and then everything appears at once.  One key at a
# time, with a pause between, is the shape that hides it.  Two numbers come out
# of each burst - how long the first character took to show, and how long after
# the last keystroke the screen stopped changing - because a terminal that
# echoes promptly and then takes seconds to catch up fails differently from one
# that shows nothing until the end.
#
# The window has to be foreground, because that is where SendInput goes, but it
# does not have to be on top.  `-ClickFraction` puts a click that far across
# the window first, for a layout where the keyboard would otherwise land in a
# sidebar's filter rather than in the grid.
param(
  [Parameter(Mandatory = $true)][int]$TargetPid,
  [int]$Samples = 20,
  [int]$Burst = 1,
  [int]$BurstGapMs = 30,
  [int]$TimeoutMs = 15000,
  [int]$QuietMs = 500,
  [double]$ClickFraction = 0,
  [string]$DumpPath = ""
)

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Win -Name Screen -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
[DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
[DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, IntPtr extra);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int left, top, right, bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int x, y; }
[StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
  public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
}
// 40 bytes on x64: the mouse arm of the union is the larger one, and SendInput
// refuses any call whose stated size disagrees with its own.
[StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT {
  [FieldOffset(0)] public uint type;
  [FieldOffset(8)] public KEYBDINPUT ki;
}
// Compiled, over one reused buffer: the same loop written in PowerShell costs
// tens of milliseconds a frame, which would become the resolution of the
// measurement rather than the thing measured.
static byte[] buf;
public static ulong Hash(IntPtr scan0, int len, int step) {
  if (buf == null || buf.Length < len) { buf = new byte[len]; }
  Marshal.Copy(scan0, buf, 0, len);
  ulong h = 14695981039346656037UL;
  for (int i = 0; i < len; i += step) { h = (h ^ buf[i]) * 1099511628211UL; }
  return h;
}
'@

if ([IntPtr]::Size -ne 8) { throw "needs 64-bit PowerShell; INPUT is 40 bytes only on x64" }

$KEYEVENTF_UNICODE = 0x0004
$KEYEVENTF_KEYUP = 0x0002
$PW_RENDERFULLCONTENT = 2

function New-UnicodeKey([char]$ch, [uint32]$extraFlags) {
  $evt = New-Object Win.Screen+INPUT
  $evt.type = 1
  $ki = New-Object Win.Screen+KEYBDINPUT
  $ki.wScan = [uint16]$ch
  $ki.dwFlags = $KEYEVENTF_UNICODE -bor $extraFlags
  $evt.ki = $ki
  return $evt
}

$down = New-UnicodeKey 'a' 0
$up = New-UnicodeKey 'a' $KEYEVENTF_KEYUP
$inputSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type]'Win.Screen+INPUT')

(New-Object -ComObject WScript.Shell).AppActivate($TargetPid) | Out-Null
Start-Sleep -Milliseconds 500

$hwnd = [Win.Screen]::GetForegroundWindow()
$owner = 0
[void][Win.Screen]::GetWindowThreadProcessId($hwnd, [ref]$owner)
if ($owner -ne $TargetPid) { throw "foreground window belongs to pid $owner, not $TargetPid" }

$rect = New-Object Win.Screen+RECT
[void][Win.Screen]::GetClientRect($hwnd, [ref]$rect)
$origin = New-Object Win.Screen+POINT
[void][Win.Screen]::ClientToScreen($hwnd, [ref]$origin)
$width = $rect.right - $rect.left
$height = $rect.bottom - $rect.top
if ($width -le 0 -or $height -le 0) { throw "window has no client area" }

if ($ClickFraction -gt 0) {
  $x = $origin.x + [int]($width * $ClickFraction)
  $y = $origin.y + [int]($height * 0.5)
  [void][Win.Screen]::SetCursorPos($x, $y)
  [Win.Screen]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  [Win.Screen]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 400
  Write-Output "clicked ($x,$y) to put the keyboard in the grid"
}

$bmp = New-Object System.Drawing.Bitmap($width, $height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$area = New-Object System.Drawing.Rectangle(0, 0, $width, $height)

$script:captureFailures = 0
function Get-Hash {
  $hdc = $gfx.GetHdc()
  # A failed capture leaves the bitmap holding the previous frame, which reads
  # as "nothing changed" and would be scored as latency rather than as a broken
  # measurement.  Counted so a run can say which it was.
  if (-not [Win.Screen]::PrintWindow($hwnd, $hdc, $PW_RENDERFULLCONTENT)) { $script:captureFailures++ }
  $gfx.ReleaseHdc($hdc)
  $data = $bmp.LockBits($area, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, $bmp.PixelFormat)
  try {
    return [Win.Screen]::Hash($data.Scan0, $data.Stride * $data.Height, 16)
  } finally {
    $bmp.UnlockBits($data)
  }
}

$timer = [System.Diagnostics.Stopwatch]::StartNew()
function Now { return $timer.Elapsed.TotalMilliseconds }

$firsts = @()
$settles = @()
$timeouts = 0
$lost = 0

for ($n = 0; $n -lt $Samples; $n++) {
  # Settle first: a burst started while the previous one is still arriving
  # measures the tail of that one.
  $stable = Get-Hash
  $settledAt = Now
  while ((Now) - $settledAt -lt $QuietMs) {
    $current = Get-Hash
    if ($current -ne $stable) { $stable = $current; $settledAt = Now }
  }

  [void][Win.Screen]::GetWindowThreadProcessId([Win.Screen]::GetForegroundWindow(), [ref]$owner)
  if ($owner -ne $TargetPid) { $lost++; continue }

  # The whole burst goes in without watching the screen: polling between keys
  # would slow the typing to the sampling rate, and the point is to type faster
  # than the terminal can keep up.
  $sentAt = Now
  for ($k = 0; $k -lt $Burst; $k++) {
    [void][Win.Screen]::SendInput(2, @($down, $up), $inputSize)
    if ($k -lt $Burst - 1) { Start-Sleep -Milliseconds $BurstGapMs }
  }
  $lastKeyAt = Now

  $firstChange = -1
  $lastChange = -1
  $current = $stable
  while ((Now) - $lastKeyAt -lt $TimeoutMs) {
    $seen = Get-Hash
    if ($seen -ne $current) {
      $current = $seen
      $lastChange = Now
      if ($firstChange -lt 0) { $firstChange = $lastChange }
    } elseif ($lastChange -ge 0 -and (Now) - $lastChange -gt $QuietMs) {
      break
    }
  }

  if ($firstChange -lt 0) {
    Write-Output "  sample ${n}: nothing appeared in ${TimeoutMs}ms"
    $timeouts++
    if ($DumpPath -and $timeouts -eq 1) { $bmp.Save("$DumpPath-timeout.png", [System.Drawing.Imaging.ImageFormat]::Png) }
    continue
  }

  $first = [math]::Round($firstChange - $sentAt, 1)
  $settle = [math]::Round($lastChange - $lastKeyAt, 1)
  Write-Output "  sample ${n}: first ${first}ms, settled ${settle}ms after the last key"
  $firsts += $first
  $settles += $settle
}

$gfx.Dispose()
$bmp.Dispose()

if ($firsts.Count -eq 0) {
  Write-Output "screen latency: no samples ($timeouts saw nothing, $lost lost focus)"
  exit 1
}
function Summarise($name, $values) {
  $sorted = $values | Sort-Object
  $q = { param($f) $sorted[[math]::Min($sorted.Count - 1, [int]($sorted.Count * $f))] }
  Write-Output ("  {0}: p50 {1}ms p95 {2}ms max {3}ms" -f $name, (& $q 0.50), (& $q 0.95), $sorted[-1])
}
Write-Output ("screen latency n={0}, burst {1} every {2}ms ({3} saw nothing, {4} lost focus, {5} captures failed)" -f `
  $firsts.Count, $Burst, $BurstGapMs, $timeouts, $lost, $script:captureFailures)
Summarise "first character" $firsts
Summarise "settled after last key" $settles
