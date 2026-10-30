param([Parameter(Mandatory)][string]$TestExe,[Parameter(Mandatory)][string]$EvidenceDirectory)
$ErrorActionPreference='Stop'
if(Test-Path -LiteralPath $EvidenceDirectory){throw 'Use a new focus evidence directory'}
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$folder=(Resolve-Path -LiteralPath $EvidenceDirectory).Path
$exe=(Resolve-Path -LiteralPath $TestExe).Path
@'
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$form=[System.Windows.Forms.Form]::new()
$form.Text='Recording external focus sentinel'
$form.ShowInTaskbar=$false
$form.FormBorderStyle='FixedToolWindow'
$form.StartPosition='Manual'
$form.SetBounds(2800,150,400,220)
$label=[System.Windows.Forms.Label]::new()
$label.Text='EXTERNAL APPLICATION INPUT FIXTURE'
$label.Dock='Fill'
$label.TextAlign='MiddleCenter'
$form.Controls.Add($label)
[System.Windows.Forms.Application]::Run($form)
'@ | Set-Content -LiteralPath (Join-Path $folder 'sentinel.ps1') -Encoding UTF8
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class RecordingFocusProbe {
 [StructLayout(LayoutKind.Sequential)] public struct Rect {public int L,T,R,B;}
 [StructLayout(LayoutKind.Sequential)] public struct Mouse {public int X,Y;public uint Data,Flags,Time;public IntPtr Extra;}
 [StructLayout(LayoutKind.Sequential)] public struct Input {public uint Type;public Mouse Mouse;}
 public delegate bool EnumWindow(IntPtr hwnd,IntPtr data);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindow callback,IntPtr data);
 [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
 public static IntPtr FindOwnedWindow(int pid){IntPtr found=IntPtr.Zero;EnumWindows((h,d)=>{uint owner;GetWindowThreadProcessId(h,out owner);if(owner==(uint)pid && IsWindowVisible(h)){found=h;return false;}return true;},IntPtr.Zero);return found;}
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
 [DllImport("user32.dll")] static extern uint SendInput(uint n,Input[] inputs,int size);
 public static void Click(IntPtr hwnd) {
  var old=SetThreadDpiAwarenessContext(new IntPtr(-4));
  try {Rect r;if(!GetWindowRect(hwnd,out r))throw new Exception("Owned sentinel unavailable");
   if(!SetCursorPos((r.L+r.R)/2,(r.T+r.B)/2))throw new Exception("Pointer placement failed");
   var inputs=new[]{new Input{Mouse=new Mouse{Flags=2}},new Input{Mouse=new Mouse{Flags=4}}};
   if(SendInput(2,inputs,Marshal.SizeOf(typeof(Input)))!=2)throw new Exception("Input rejected");
  }finally{SetThreadDpiAwarenessContext(old);}
 }
}
'@
$sentinel=$null;$sentinelHwnd=[IntPtr]::Zero;$recording=$null;$saved=$env:LEXIFT_RECORDING_GATE_DIR
try {
    $sentinel=Start-Process C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $folder 'sentinel.ps1')) -WindowStyle Hidden -PassThru
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do{$sentinel.Refresh();if($sentinel.HasExited){throw 'Sentinel exited'};if(($sentinelHwnd=[RecordingFocusProbe]::FindOwnedWindow($sentinel.Id)) -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 20}while([DateTime]::UtcNow -lt $deadline)
    if($sentinelHwnd -eq 0){throw 'Sentinel was not prepared'}
    @{roi=@(320,200,2240,1280);rounds=1;spec=@{dynamic_ms=5000;static_ms=1000;pause_ms=500;resume_ms=1000}} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $folder 'plan.json') -Encoding utf8NoBOM
    Get-FileHash $exe -Algorithm SHA256 | ConvertTo-Json | Set-Content (Join-Path $folder 'build.json') -Encoding utf8NoBOM
    $env:LEXIFT_RECORDING_GATE_DIR=$folder
    $recording=Start-Process $exe -ArgumentList @('windows::recording_gate::native_recording_matrix','--exact','--ignored','--nocapture') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $folder 'native-test.log') -RedirectStandardError (Join-Path $folder 'native-error.log')
    $samples=Join-Path $folder 'round-00.partial.samples.jsonl'
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do{if($recording.HasExited){throw 'Recording exited before focus probe'};if((Test-Path $samples) -and (Get-Item $samples).Length -gt 0){break};Start-Sleep -Milliseconds 20}while([DateTime]::UtcNow -lt $deadline)
    if(-not (Test-Path $samples)){throw 'Recording never accepted pixels'}
    [RecordingFocusProbe]::Click($sentinelHwnd)
    $deadline=[DateTime]::UtcNow.AddSeconds(2)
    do{if([RecordingFocusProbe]::GetForegroundWindow() -eq $sentinelHwnd){break};Start-Sleep -Milliseconds 10}while([DateTime]::UtcNow -lt $deadline)
    if([RecordingFocusProbe]::GetForegroundWindow() -ne $sentinelHwnd){throw 'Real input did not activate the external sentinel'}
    if(-not $recording.WaitForExit(20000)){throw 'Recording did not retire'}
    if($recording.ExitCode -ne 0){throw 'Native recording failed'}
    $actual=[RecordingFocusProbe]::GetForegroundWindow()
    @{expected=$sentinelHwnd.ToInt64();actual=$actual.ToInt64();retained=($actual -eq $sentinelHwnd);activation='real mouse input; no SetForegroundWindow'} | ConvertTo-Json | Set-Content (Join-Path $folder 'foreground.json') -Encoding utf8NoBOM
    if($actual -ne $sentinelHwnd){throw 'Recording reclaimed external foreground'}
} finally {
    $env:LEXIFT_RECORDING_GATE_DIR=$saved
    if($recording -and -not $recording.HasExited){Stop-Process -Id $recording.Id}
    if($sentinel -and -not $sentinel.HasExited){$null=$sentinel.CloseMainWindow();if(-not $sentinel.WaitForExit(3000)){Stop-Process -Id $sentinel.Id}}
}
