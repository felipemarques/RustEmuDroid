[CmdletBinding()]
param(
    [string] $Images = '.rustemudroid/bluestacks-lab/Pie64',
    [string] $Output,
    [ValidateNotNullOrEmpty()]
    [string] $Distribution = 'Ubuntu-24.04',
    [ValidateRange(1, 120)]
    [int] $Seconds = 40,
    [switch] $VerifyHashes
)

$ErrorActionPreference = 'Stop'
$imageDirectory = Get-Item -LiteralPath $Images
if (-not $imageDirectory.PSIsContainer -or ($imageDirectory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Expected an ordinary directory with the copied Pie64 images.'
}
$lab = Split-Path -Parent $imageDirectory.FullName
$target = if ($Output) {
    $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
} else {
    Join-Path $lab "data-probe-$(Get-Date -Format yyyyMMdd-HHmmss)-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
}

$manifestPath = Join-Path $imageDirectory.FullName 'manifest.json'
$manifest = if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
} elseif ($VerifyHashes) {
    throw "Manifest verification requires $manifestPath."
}
foreach ($name in 'fastboot.vdi', 'Data_orig.vhdx') {
    $copy = Get-Item -LiteralPath (Join-Path $imageDirectory.FullName $name)
    $entry = @($manifest | Where-Object file -CEQ $name)
    if ($copy.PSIsContainer -or ($copy.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $copy.Length -le 0 -or ($null -ne $manifest -and ($entry.Count -ne 1 -or $copy.Length -ne $entry[0].bytes))) {
        throw "Copy verification failed for $name. Check the copied files and manifest."
    }
    if ($VerifyHashes -and (Get-FileHash -LiteralPath $copy.FullName -Algorithm SHA256).Hash -ne $entry[0].sha256) {
        throw "Manifest SHA-256 mismatch for $name."
    }
}

function Convert-ToLinuxPath([string] $WindowsPath) {
    $mapped = & wsl.exe --distribution $Distribution --exec wslpath -a -u ($WindowsPath.Replace('\', '/'))
    if ($LASTEXITCODE -ne 0 -or -not $mapped) {
        throw "Could not map $WindowsPath into WSL ($Distribution)."
    }
    $mapped.Trim()
}

$wslArgs = @('--distribution', $Distribution, '--cd', $PSScriptRoot, '--exec')
$probeArgs = @('python3', '-B', 'bluestacks_data_probe.py',
    '--images', (Convert-ToLinuxPath $imageDirectory.FullName), '--output', (Convert-ToLinuxPath $target),
    '--seconds', "$Seconds")
$toolsDirectory = Join-Path $lab 'qemu-tools'
$qemu = 'usr/bin/qemu-system-x86_64'
$qemuImg = 'usr/bin/qemu-img'
$bios = 'usr/share/seabios/bios-256k.bin'
if ((Test-Path -LiteralPath (Join-Path $toolsDirectory $qemu) -PathType Leaf) -and
    (Test-Path -LiteralPath (Join-Path $toolsDirectory $qemuImg) -PathType Leaf) -and
    (Test-Path -LiteralPath (Join-Path $toolsDirectory $bios) -PathType Leaf)) {
    $tools = Convert-ToLinuxPath $toolsDirectory
    $libPath = "$tools/usr/lib/x86_64-linux-gnu:${tools}/lib/x86_64-linux-gnu"
    $wslArgs += @('env', "LD_LIBRARY_PATH=$libPath", "QEMU_MODULE_DIR=$tools/usr/lib/x86_64-linux-gnu/qemu")
    $probeArgs += @('--qemu', "$tools/$qemu", '--qemu-img', "$tools/$qemuImg",
        '--firmware', "$tools/usr/share/seabios", '--bios', "$tools/$bios")
    Write-Host "Using local QEMU tools from $toolsDirectory."
}

Write-Host "Running the headless read-only data probe in WSL ($Distribution)."
& wsl.exe @wslArgs @probeArgs
if ($LASTEXITCODE -ne 0) {
    throw "The data probe failed with exit code $LASTEXITCODE. Check $target for evidence."
}
