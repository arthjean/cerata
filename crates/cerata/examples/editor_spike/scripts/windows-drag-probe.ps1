# US-002 Windows drag probe (throwaway). Drags with the real mouse from the
# start of a paragraph with hidden `**` and link URL, across a block image,
# into the line under it, and captures the window while pressed (hide
# decisions frozen) and after release (nodes revealed). Window coordinates
# assume a 1200x800 window at scale factor 1.
#
#   powershell -ExecutionPolicy Bypass -File windows-drag-probe.ps1 -exe TARGET_DIR\release\examples\editor_spike.exe -sp OUT_DIR
param([string]$exe, [string]$sp)
$ErrorActionPreference = 'Stop'
& $exe --write-fixture $sp | Out-Null
$note = Join-Path $sp 'image-check.md'
[IO.File]::WriteAllText($note, "# Image check`n`nA paragraph with **bold** text and a [link](https://example.com/x) before the figure.`n![Figure](images/figure-2.svg)`nText right under the figure, which is 800x333 scaled to the 720 px column: 300 px, not a multiple of 30.`n`n![Figure](images/figure-0.svg)`nlast line`n")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class D {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  public struct RECT { public int L, T, R, B; }
}
"@
[D]::SetProcessDPIAware() | Out-Null
$env:RUST_LOG = 'info'
$proc = Start-Process -FilePath $exe -ArgumentList "`"$note`"" -PassThru -RedirectStandardError "$sp\drag.log"
Start-Sleep -Seconds 3
$h = $proc.MainWindowHandle
function Shot($name) {
  $r = New-Object D+RECT; [D]::GetWindowRect($h, [ref]$r) | Out-Null
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  $g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $bmp.Save("$sp\$name.png")
}
try {
  $r = New-Object D+RECT; [D]::GetWindowRect($h, [ref]$r) | Out-Null
  # Focus click far below the text, then drag from the paragraph start to the
  # middle of the line under the first figure.
  [D]::SetCursorPos($r.L + 600, $r.B - 40); [D]::mouse_event(2,0,0,0,[UIntPtr]::Zero); [D]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
  Start-Sleep -Milliseconds 400
  [D]::SetCursorPos($r.L + 249, $r.T + 161); Start-Sleep -Milliseconds 100
  [D]::mouse_event(2,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 100
  for ($y = 161; $y -le 491; $y += 15) { [D]::SetCursorPos($r.L + 249 + ($y - 161) / 2, $r.T + $y); Start-Sleep -Milliseconds 30 }
  [D]::SetCursorPos($r.L + 420, $r.T + 491); Start-Sleep -Milliseconds 300
  Shot "drag-pressed"
  [D]::mouse_event(4,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 400
  Shot "drag-released"
} finally { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
