# Rune Kit per-user installer. PowerShell 5.1+; no administrator rights required.
# irm https://raw.githubusercontent.com/shba007/rune-kit/main/install.ps1 | iex
# Overrides: RUNE_KIT_VERSION, RUNE_KIT_INSTALL_DIR, RUNE_KIT_BASE_URL,
# RUNE_KIT_NO_MODIFY_PATH=1. Local mirrors require a pinned version + .sha256.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    function Setting($name, $fallback) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ([string]::IsNullOrWhiteSpace($value)) { return $fallback }
        return $value
    }
    $version = Setting 'RUNE_KIT_VERSION' 'latest'
    $base = Setting 'RUNE_KIT_BASE_URL' ''
    $userHome = [Environment]::GetFolderPath('UserProfile')
    if ([string]::IsNullOrWhiteSpace($userHome)) { throw 'Cannot resolve user home.' }
    $installDir = [IO.Path]::GetFullPath((Setting 'RUNE_KIT_INSTALL_DIR' (Join-Path $userHome 'rune')))
    $release = $null
    if ($base) {
        if ($version -eq 'latest') { throw 'Set RUNE_KIT_VERSION when using a mirror.' }
    } else {
        $endpoint = 'https://api.github.com/repos/shba007/rune-kit/releases/latest'
        if ($version -ne 'latest') {
            if (-not $version.StartsWith('v')) { $version = "v$version" }
            $endpoint = "https://api.github.com/repos/shba007/rune-kit/releases/tags/$version"
        }
        $release = Invoke-RestMethod -Uri $endpoint -Headers @{ 'User-Agent' = 'rune-kit-installer' }
        $version = $release.tag_name
        $base = "https://github.com/shba007/rune-kit/releases/download/$version"
    }
    if (-not $version.StartsWith('v')) { $version = "v$version" }
    if ($version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*$') { throw 'Invalid release version.' }
    $base = $base.TrimEnd('/', '\')

    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
    try { $arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch {}
    if ($arch -eq 'Arm64') {
        Write-Warning 'Using the x64 package under Windows on Arm emulation; no native Arm64 package is published.'
    } elseif ($arch -notin @('X64', 'AMD64')) { throw "Unsupported Windows architecture: $arch" }
    $target = 'x86_64-pc-windows-msvc'

    function Fetch($name, $destination) {
        if (Test-Path -LiteralPath $base -PathType Container) {
            Copy-Item -LiteralPath (Join-Path $base $name) -Destination $destination
        } else {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $destination
        }
    }
    $archive = "rune-kit-$version-$target.zip"
    $asset = $null
    if ($release) {
        $asset = $release.assets | Where-Object { $_.name -eq $archive } | Select-Object -First 1
        if (-not $asset) {
            $archive = "rune-$version-$target.zip"
            $asset = $release.assets | Where-Object { $_.name -eq $archive } | Select-Object -First 1
        }
        if (-not $asset) { throw 'Windows package is not available yet. The release may still be building; try again later.' }
    } elseif (Test-Path -LiteralPath $base -PathType Container) {
        if (-not (Test-Path -LiteralPath (Join-Path $base $archive))) {
            $archive = "rune-$version-$target.zip"
        }
    }

    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $temp = Join-Path $tempRoot ('rune-kit-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temp | Out-Null
    $stage = $null
    $backup = $null
    try {
        Write-Host "Installing Rune Kit $version..."
        $zipPath = Join-Path $temp $archive
        Fetch $archive $zipPath
        $expected = $null
        $hasSidecar = -not $release -or ($release.assets | Where-Object { $_.name -eq "$archive.sha256" })
        if ($hasSidecar) {
            $checksum = Join-Path $temp 'checksum.txt'
            Fetch "$archive.sha256" $checksum
            $expected = ((Get-Content -LiteralPath $checksum -Raw).Trim() -split '\s+')[0]
        } elseif ($asset.digest -match '^sha256:([a-fA-F0-9]{64})$') {
            $expected = $Matches[1]
        } else { throw 'No SHA-256 checksum is available. Installation stopped without replacing the existing binary.' }
        if ($expected -notmatch '^[a-fA-F0-9]{64}$') { throw 'Invalid SHA-256 checksum.' }
        $sha256 = [Security.Cryptography.SHA256]::Create()
        $archiveStream = [IO.File]::OpenRead($zipPath)
        try { $actual = [BitConverter]::ToString($sha256.ComputeHash($archiveStream)).Replace('-', '') }
        finally { $archiveStream.Dispose(); $sha256.Dispose() }
        if ($actual -ne $expected) { throw 'Checksum mismatch. Installation stopped without replacing the existing binary.' }

        # Read only the expected root executable, never arbitrary archive paths.
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($zipPath)
        $candidate = Join-Path $temp 'rune-kit.exe'
        try {
            $entry = $zip.GetEntry('rune-kit.exe')
            if (-not $entry) { $entry = $zip.GetEntry('rune.exe') }
            if (-not $entry) { throw 'Archive contains neither rune-kit.exe nor legacy rune.exe.' }
            $inputStream = $entry.Open()
            $outputStream = [IO.File]::Create($candidate)
            try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose(); $inputStream.Dispose() }
        } finally { $zip.Dispose() }
        $reported = & $candidate --version
        if ($LASTEXITCODE -ne 0 -or "$reported" -notmatch '^(rune-kit|rune)\s+\S+$') { throw 'Downloaded executable failed its version check.' }
        $expectedVersion = $version.Substring(1)
        if (("$reported" -split '\s+')[1] -ne $expectedVersion) { throw "Downloaded binary reports an unexpected version: $reported" }

        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        $destination = Join-Path $installDir 'rune-kit.exe'
        $stage = Join-Path $installDir ('.rune-kit-install-' + [Guid]::NewGuid().ToString('N') + '.tmp')
        Copy-Item -LiteralPath $candidate -Destination $stage
        if (Test-Path -LiteralPath $destination) {
            $backup = Join-Path $installDir ('.rune-kit-old-' + [Guid]::NewGuid().ToString('N') + '.exe')
            Move-Item -LiteralPath $destination -Destination $backup
        }
        try { Move-Item -LiteralPath $stage -Destination $destination }
        catch {
            if ($backup -and -not (Test-Path -LiteralPath $destination)) { Move-Item -LiteralPath $backup -Destination $destination }
            throw
        }
        if ($backup) {
            try { Remove-Item -LiteralPath $backup -Force } catch { Write-Warning "Close the old gateway, then remove $backup." }
        }

        if ((Setting 'RUNE_KIT_NO_MODIFY_PATH' '0') -ne '1') {
            $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            $entries = @($userPath -split ';' | Where-Object { $_ })
            if (($entries | ForEach-Object { $_.TrimEnd('\') }) -notcontains $installDir.TrimEnd('\')) {
                [Environment]::SetEnvironmentVariable('Path', (($entries + $installDir) -join ';'), 'User')
            }
            if ((@($env:Path -split ';') | ForEach-Object { $_.TrimEnd('\') }) -notcontains $installDir.TrimEnd('\')) {
                $env:Path = "$installDir;$env:Path"
            }
            Write-Host 'User PATH updated. Restart other terminals and MCP clients to use it.'
        } else { Write-Host "PATH unchanged. Add $installDir yourself or use the full executable path." }
        Write-Host "Installed: $destination ($reported)"
        if ("$reported" -match '^rune\s') { Write-Host 'This older release retains its original display name; the installed command is rune-kit.' }
        Write-Host 'Next: rune-kit --help'
    } finally {
        if ($stage -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Force }
        $resolvedTemp = [IO.Path]::GetFullPath($temp)
        # Delete only the unique staging directory created above under the OS temp root.
        if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
            ([IO.Path]::GetFileName($resolvedTemp)).StartsWith('rune-kit-')) {
            Remove-Item -LiteralPath $resolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
