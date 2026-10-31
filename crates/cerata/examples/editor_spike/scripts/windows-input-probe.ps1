# US-002 Windows input probe (throwaway). Drives real OS keyboard input into
# the spike: AZERTY dead circumflex then e, then MS-IME Japanese "nihongo",
# space, enter. Needs the fr-FR (040C) and ja-JP MS-IME layouts installed.
# Moves the mouse and types: run it with nobody using the machine. Key
# injection aborts if the spike window loses the foreground.
#
#   cargo build -p cerata --example editor_spike --release
#   powershell -ExecutionPolicy Bypass -File windows-input-probe.ps1 -exe TARGET_DIR\release\examples\editor_spike.exe -sp OUT_DIR
#
# Then read OUT_DIR\probe.log (every EntityInputHandler call is logged at
# debug level under the `spike_input` target) and the probe-*.png captures.
#
# -ime pinyin runs Microsoft Pinyin instead of MS-IME (needs zh-CN in the
# user language list): types "zhongwen", captures, then picks candidate 1.
# On 2026-10-31 Pinyin activated (taskbar 拼) but stayed in English mode (英)
# under injected input, so the letters arrived as plain text: none of the
# mode switches below (WM_IME_CONTROL, and before it Shift, VK_IME_ON,
# Ctrl+Space) changed that. Pinyin needs a human at the keyboard.
param([string]$exe, [string]$sp, [string]$ime = 'ja')
$ErrorActionPreference = 'Stop'
$note = Join-Path $sp 'probe.md'
[IO.File]::WriteAllText($note, "## IME probe`n`nType after this: ")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class P {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr LoadKeyboardLayout(string id, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("imm32.dll")] public static extern IntPtr ImmGetDefaultIMEWnd(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct RECT { public int L, T, R, B; }
}
"@
[P]::SetProcessDPIAware() | Out-Null
function Shot($name, $extraBelow) {
  $r = New-Object P+RECT; [P]::GetWindowRect($script:h, [ref]$r) | Out-Null
  $w = $r.R - $r.L; $hh = $r.B - $r.T + $extraBelow
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $bmp.Save("$sp\$name.png")
}
function Fg { if ([P]::GetForegroundWindow() -ne $script:h) { throw "spike window lost the foreground, aborting key injection" } }
function Key($scan, $vk = 0) {
  Fg
  if ($vk -ne 0) { [P]::keybd_event($vk, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30; [P]::keybd_event($vk, 0, 2, [UIntPtr]::Zero) }
  else { [P]::keybd_event(0, $scan, 8, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30; [P]::keybd_event(0, $scan, 10, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 120
}
$env:RUST_LOG = 'info,spike_input=debug'
$proc = Start-Process -FilePath $exe -ArgumentList "`"$note`"" -PassThru -RedirectStandardError "$sp\probe.log"
Start-Sleep -Seconds 3
$script:h = $proc.MainWindowHandle
try {
  $r = New-Object P+RECT; [P]::GetWindowRect($h, [ref]$r) | Out-Null
  # Click in the empty area below the text: the caret goes to the end of the note.
  [P]::SetCursorPos($r.L + 600, $r.B - 120); Start-Sleep -Milliseconds 100
  [P]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50; [P]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 500
  Fg
  # French AZERTY: dead circumflex (scan 0x1A) then e (scan 0x12).
  $fr = [P]::LoadKeyboardLayout("0000040C", 1)
  [P]::PostMessage($h, 0x50, [IntPtr]::Zero, $fr) | Out-Null; Start-Sleep -Milliseconds 400
  Key 0x1A; Key 0x12; Key 0x39
  Start-Sleep -Milliseconds 300
  if ($ime -eq 'pinyin') {
    # Microsoft Pinyin: switch to zh-CN, type "zhongwen", pick candidate 1.
    $zh = [P]::LoadKeyboardLayout("00000804", 1)
    [P]::PostMessage($h, 0x50, [IntPtr]::Zero, $zh) | Out-Null; Start-Sleep -Milliseconds 800
    # Microsoft Pinyin starts in English mode (taskbar shows 英). Open the
    # IME and set native (Chinese) conversion through the window's default
    # IME window: WM_IME_CONTROL, IMC_SETOPENSTATUS then IMC_SETCONVERSIONMODE.
    $imeWnd = [P]::ImmGetDefaultIMEWnd($h)
    [P]::SendMessage($imeWnd, 0x283, [IntPtr]0x6, [IntPtr]1) | Out-Null
    [P]::SendMessage($imeWnd, 0x283, [IntPtr]0x2, [IntPtr]1) | Out-Null
    Start-Sleep -Milliseconds 300
    foreach ($s in 0x2C, 0x23, 0x18, 0x31, 0x22, 0x11, 0x12, 0x31) { Key $s }
    Start-Sleep -Milliseconds 500
    Shot "pinyin-composing" 250
    Key 0x02
    Start-Sleep -Milliseconds 400
    [P]::PostMessage($h, 0x50, [IntPtr]::Zero, $fr) | Out-Null
    Start-Sleep -Milliseconds 300
    Shot "pinyin-final" 0
    return
  }
  # Japanese MS-IME: switch layout, IME on, type "nihongo", space to convert, enter to commit.
  $ja = [P]::LoadKeyboardLayout("00000411", 1)
  [P]::PostMessage($h, 0x50, [IntPtr]::Zero, $ja) | Out-Null; Start-Sleep -Milliseconds 600
  Key 0 0x16
  foreach ($s in 0x31, 0x17, 0x23, 0x18, 0x31, 0x22, 0x18) { Key $s }
  Start-Sleep -Milliseconds 300
  Shot "probe-composing" 0
  Key 0x39
  Start-Sleep -Milliseconds 500
  Shot "probe-converting" 250
  Key 0x1C
  Start-Sleep -Milliseconds 300
  Key 0 0x1A
  [P]::PostMessage($h, 0x50, [IntPtr]::Zero, $fr) | Out-Null
  Start-Sleep -Milliseconds 300
  Shot "probe-final" 0
} finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
