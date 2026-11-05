# P4-4 evidence helper: parse a Windows command line with the *system* rules.
#
#   powershell -File commandline-argv.ps1 -CommandLine '"C:\Program Files\Hangline\hangline.exe" --flag'
#   -> ["C:\\Program Files\\Hangline\\hangline.exe","--flag"]
#
# Used by the autostart harness so the Run-value assertion is not a hand-rolled approximation:
# `CommandLineToArgvW` is the same API Windows/`CreateProcess` callers use to split the value.
param(
    [Parameter(Mandatory = $true)][string]$CommandLine
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace HanglineP4 -Name Native -MemberDefinition @'
[DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern IntPtr CommandLineToArgvW(string lpCmdLine, out int pNumArgs);

[DllImport("kernel32.dll")]
public static extern IntPtr LocalFree(IntPtr hMem);
'@

$argc = 0
$argv = [HanglineP4.Native]::CommandLineToArgvW($CommandLine, [ref]$argc)
if ($argv -eq [IntPtr]::Zero) {
    $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    throw "CommandLineToArgvW failed for the login-item value (Win32 error $code)"
}

try {
    $items = @(
        for ($i = 0; $i -lt $argc; $i++) {
            $pointer = [Runtime.InteropServices.Marshal]::ReadIntPtr($argv, $i * [IntPtr]::Size)
            [Runtime.InteropServices.Marshal]::PtrToStringUni($pointer)
        }
    )
    # PowerShell 5.1 serializes a one-element array as a scalar; build the JSON array explicitly.
    $encoded = @($items | ForEach-Object { ConvertTo-Json -Compress -InputObject ([string]$_) })
    Write-Output ("[" + ($encoded -join ",") + "]")
}
finally {
    [HanglineP4.Native]::LocalFree($argv) | Out-Null
}
