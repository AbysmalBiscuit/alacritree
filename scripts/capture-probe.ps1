# Does screen capture actually see this window's pixels?
#
# A hardware-accelerated window does not always come back from a BitBlt of the
# screen DC - the result can be black, or the desktop behind it.  A latency
# bench built on capture is worthless until that is settled, so this writes one
# frame to a file and reports whether two captures a second apart differ.
param(
  [Parameter(Mandatory = $true)][int]$TargetPid,
  [Parameter(Mandatory = $true)][string]$OutPath
)

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace Win -Name Cap -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
[DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int left, top, right, bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int x, y; }
static byte[] buf;
public static ulong Hash(IntPtr scan0, int len, int step) {
  if (buf == null || buf.Length < len) { buf = new byte[len]; }
  Marshal.Copy(scan0, buf, 0, len);
  ulong h = 14695981039346656037UL;
  for (int i = 0; i < len; i += step) { h = (h ^ buf[i]) * 1099511628211UL; }
  return h;
}
'@

(New-Object -ComObject WScript.Shell).AppActivate($TargetPid) | Out-Null
Start-Sleep -Milliseconds 800

$hwnd = [Win.Cap]::GetForegroundWindow()
$owner = 0
[void][Win.Cap]::GetWindowThreadProcessId($hwnd, [ref]$owner)
Write-Output "foreground pid $owner (target $TargetPid), hwnd $hwnd"

$rect = New-Object Win.Cap+RECT
[void][Win.Cap]::GetClientRect($hwnd, [ref]$rect)
$origin = New-Object Win.Cap+POINT
[void][Win.Cap]::ClientToScreen($hwnd, [ref]$origin)
$w = $rect.right - $rect.left
$h = $rect.bottom - $rect.top
Write-Output "client ${w}x${h} at ($($origin.x),$($origin.y))"

$bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$area = New-Object System.Drawing.Rectangle(0, 0, $w, $h)
$size = New-Object System.Drawing.Size($w, $h)

function Hash-Bitmap {
  $data = $bmp.LockBits($area, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, $bmp.PixelFormat)
  try { return [Win.Cap]::Hash($data.Scan0, $data.Stride * $data.Height, 16) }
  finally { $bmp.UnlockBits($data) }
}

# BitBlt of the screen DC first, then PrintWindow with PW_RENDERFULLCONTENT,
# which is the one that reaches a window drawing through its own swap chain.
foreach ($method in 'CopyFromScreen', 'PrintWindow') {
  $hashes = @()
  foreach ($i in 1, 2) {
    if ($method -eq 'CopyFromScreen') {
      $gfx.CopyFromScreen($origin.x, $origin.y, 0, 0, $size)
    } else {
      $hdc = $gfx.GetHdc()
      [void][Win.Cap]::PrintWindow($hwnd, $hdc, 2)
      $gfx.ReleaseHdc($hdc)
    }
    $hashes += Hash-Bitmap
    if ($i -eq 1) { Start-Sleep -Milliseconds 900 }
  }
  $verdict = if ($hashes[0] -eq $hashes[1]) { 'IDENTICAL over 900ms' } else { 'changed' }
  Write-Output "$method : $($hashes[0]) then $($hashes[1]) - $verdict"
  $bmp.Save("$OutPath-$method.png", [System.Drawing.Imaging.ImageFormat]::Png)
}

$gfx.Dispose()
$bmp.Dispose()
Write-Output "wrote $OutPath-CopyFromScreen.png and $OutPath-PrintWindow.png"
