[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()]
    [string] $Distribution = 'Ubuntu-24.04',

    [ValidateRange(1, 120)]
    [int] $Seconds = 40,

    [switch] $VerifyHashes,

    [switch] $LegacyStorage
)

$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$lab = '.rustemudroid/bluestacks-lab'
$images = "$lab/Pie64"
$tools = "$lab/qemu-tools"
$manifestPath = Join-Path $project "$images/manifest.json"
foreach ($directory in '.rustemudroid', $lab, $images) {
    $item = Get-Item -LiteralPath (Join-Path $project $directory)
    if (-not $item.PSIsContainer -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw "Expected an ordinary directory in the lab: $directory"
    }
}
$manifest = if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
} elseif ($VerifyHashes) {
    throw "Hash verification requires the offline copies' SHA-256 manifest at $manifestPath."
}
if ($VerifyHashes) {
    Write-Host 'Verifying SHA-256 hashes. Hashing the data disk can take a moment.'
}
foreach ($name in 'fastboot.vdi', 'Root.vhd', 'Data.vhdx') {
    $entry = @($manifest | Where-Object file -CEQ $name)
    $disk = Get-Item -LiteralPath (Join-Path $project "$images/$name")
    if ($disk.PSIsContainer -or
        ($disk.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -or
        $disk.Length -le 0 -or
        ($manifest -and ($entry.Count -ne 1 -or $disk.Length -ne $entry[0].bytes))) {
        throw "Copy verification failed for $name. Check the offline copies and manifest."
    }
    if ($VerifyHashes -and
        (Get-FileHash -LiteralPath $disk.FullName -Algorithm SHA256).Hash -ne $entry[0].sha256) {
        throw "SHA-256 verification failed for $name."
    }
}

$run = "$lab/direct-linux-$(Get-Date -Format yyyyMMdd-HHmmss)-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$wslArgs = @('--distribution', $Distribution, '--cd', $project, '--exec')
$probeArgs = @('python3', 'tools/bluestacks_lab.py', '--images', $images,
    '--output', $run, '--direct-linux', '--seconds', "$Seconds")
if (-not $LegacyStorage) {
    $probeArgs += '--virtio-blk'
}
$qemu = "$tools/usr/bin/qemu-system-x86_64"
$qemuImg = "$tools/usr/bin/qemu-img"
$bios = "$tools/usr/share/seabios/bios-256k.bin"
if ((Test-Path -LiteralPath (Join-Path $project $qemu) -PathType Leaf) -and
    (Test-Path -LiteralPath (Join-Path $project $qemuImg) -PathType Leaf) -and
    (Test-Path -LiteralPath (Join-Path $project $bios) -PathType Leaf)) {
    $libPath = "$tools/usr/lib/x86_64-linux-gnu:${tools}/lib/x86_64-linux-gnu"
    $wslArgs += @('env', "LD_LIBRARY_PATH=$libPath", "QEMU_MODULE_DIR=$tools/usr/lib/x86_64-linux-gnu/qemu")
    $probeArgs += @('--qemu', $qemu, '--qemu-img', $qemuImg,
        '--firmware', "$tools/usr/share/seabios", '--bios', $bios)
}

Write-Host "Running the headless Linux kernel diagnostic in WSL ($Distribution)."
& wsl.exe @wslArgs @probeArgs
$probeExit = $LASTEXITCODE
$output = Join-Path $project $run
if (Test-Path -LiteralPath $output -PathType Container) {
    Write-Host "Evidence folder $output"
    foreach ($name in 'serial.log', 'qemu.log', 'result.json') {
        $log = Join-Path $output $name
        if (Test-Path -LiteralPath $log -PathType Leaf) {
            Write-Host $log
        }
    }
}
if ($probeExit -ne 0) {
    throw "Evidence collection failed with exit code $probeExit. Check the QEMU log if available."
}

$serial = Join-Path $output 'serial.log'
if ((Test-Path -LiteralPath $serial -PathType Leaf) -and
    (Select-String -LiteralPath $serial -Pattern 'Cannot load module /boot/bstmods/bstinput\.ko' -Quiet)) {
    Write-Host 'The kernel reached initramfs userspace. The BlueStacks input module failed to load. Android startup was not verified.'
} elseif ((Test-Path -LiteralPath $serial -PathType Leaf) -and
    (Select-String -LiteralPath $serial -Pattern 'watchdog:.*soft lockup' -Quiet)) {
    Write-Host 'The guest kernel reported a watchdog soft lockup. Android startup was not verified.'
} else {
    Write-Host 'Diagnostic complete. Inspect the serial log for the last guest boot stage. Android startup was not verified.'
}
