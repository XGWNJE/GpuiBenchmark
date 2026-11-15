[CmdletBinding()]
param(
    [ValidateSet('Release', 'Debug')]
    [string]$Configuration = 'Release',
    [ValidateSet('arm64-v8a', 'x86_64')]
    [string]$Abi = 'arm64-v8a',
    [string]$SdkRoot = $env:ANDROID_HOME,
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$buildRoot = Join-Path $repoRoot '.build'
$buildBudget = 12GB
$lockPath = Join-Path $repoRoot '.build.lock'
$lock = $null
$savedEnvironment = @{}
$ownedBuildDirectories = @('cargo', 'gradle', 'gradle-cache', 'jniLibs', 'logs')

function Get-BuildDirectories {
    $expectedRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.build'))
    if (Test-Path -LiteralPath $expectedRoot) {
        $rootItem = Get-Item -LiteralPath $expectedRoot -Force
        if ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'The build root is a linked directory. Check its ownership before cleaning.'
        }
    }
    $rootPrefix = $expectedRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    foreach ($directoryName in $ownedBuildDirectories) {
        $directory = [IO.Path]::GetFullPath((Join-Path $expectedRoot $directoryName))
        if (-not $directory.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Build directory is outside the repository build root: $directory"
        }
        if (Test-Path -LiteralPath $directory) {
            $directoryItem = Get-Item -LiteralPath $directory -Force
            if ($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Build directory is linked. Check its ownership before cleaning: $directory"
            }
            $resolvedDirectory = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $directory).ProviderPath)
            if (-not $resolvedDirectory.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Resolved build directory is outside the repository build root: $resolvedDirectory"
            }
            $resolvedDirectory
        }
    }
}

function Get-BuildBytes {
    $total = 0L
    foreach ($directory in Get-BuildDirectories) {
        $total += [long](Get-ChildItem -LiteralPath $directory -File -Recurse -Force -ErrorAction SilentlyContinue |
            Measure-Object -Property Length -Sum).Sum
    }
    return $total
}

function Assert-NoRustBuild {
    $repositoryPath = $repoRoot.Replace('\', '/')
    $activeProcesses = Get-CimInstance Win32_Process -Filter "Name = 'cargo.exe' OR Name = 'rustc.exe' OR Name = 'rustbuild.exe'"
    foreach ($activeProcess in $activeProcesses) {
        $command = [string]$activeProcess.CommandLine
        if ($command.Replace('\', '/').IndexOf($repositoryPath, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "A Rust build is using this repository (PID $($activeProcess.ProcessId))."
        }
    }
    $cargoDirectory = Join-Path $buildRoot 'cargo'
    if (-not (Test-Path -LiteralPath $cargoDirectory)) { return }
    foreach ($cargoLock in Get-ChildItem -LiteralPath $cargoDirectory -Filter '.cargo-lock' -File -Recurse -Force) {
        $cargoLockStream = $null
        try {
            $cargoLockStream = [IO.File]::Open($cargoLock.FullName, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
            $cargoLockStream.Lock(0, 1)
            $cargoLockStream.Unlock(0, 1)
        }
        catch { throw "A Rust build holds a Cargo target lock: $($cargoLock.FullName)" }
        finally { if ($cargoLockStream) { $cargoLockStream.Dispose() } }
    }
}

function Assert-BuildBudget {
    $buildBytes = Get-BuildBytes
    if ($buildBytes -gt $buildBudget) {
        throw ('Build cache exceeds 12 GiB ({0:N2} GiB). Run this script with -Clean, then rebuild.' -f ($buildBytes / 1GB))
    }
}

function Set-BuildEnvironment([string]$Name, [string]$Value) {
    if (-not $savedEnvironment.ContainsKey($Name)) {
        $savedEnvironment[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
    }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}

function Invoke-BuildProcess([string]$Executable, [string]$Arguments, [string]$Stage) {
    Assert-BuildBudget
    $logRoot = Join-Path $buildRoot 'logs'
    New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
    $stdout = Join-Path $logRoot "$Stage.out.log"
    $stderr = Join-Path $logRoot "$Stage.err.log"
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WorkingDirectory $repoRoot `
        -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        while (-not $process.WaitForExit(5000)) {
            Assert-BuildBudget
            if ($timer.Elapsed.TotalMinutes -gt 60) {
                throw "$Stage exceeded the 60-minute build limit. Logs remain in .build/logs."
            }
        }
        $process.WaitForExit()
        Get-Content -LiteralPath $stdout -ErrorAction SilentlyContinue | Write-Output
        Get-Content -LiteralPath $stderr -ErrorAction SilentlyContinue | Write-Output
        if ($process.ExitCode -ne 0) { throw "$Stage failed with exit code $($process.ExitCode)." }
        Assert-BuildBudget
    }
    finally {
        if (-not $process.HasExited) {
            & taskkill.exe /PID $process.Id /T /F | Out-Null
        }
        $process.Dispose()
    }
}

try {
    try {
        $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    }
    catch { throw 'Another GPUI benchmark build is using this repository.' }

    if ($Clean) {
        $buildDirectories = @(Get-BuildDirectories)
        Assert-NoRustBuild
        foreach ($directory in $buildDirectories) {
            Remove-Item -LiteralPath $directory -Recurse -Force
        }
        Write-Output 'Removed repository-owned Cargo, Gradle, JNI, and build-log directories.'
        return
    }

    if (-not $SdkRoot) { $SdkRoot = $env:ANDROID_SDK_ROOT }
    if (-not $SdkRoot) { throw 'Set ANDROID_HOME or pass -SdkRoot with an Android SDK path.' }
    $SdkRoot = [IO.Path]::GetFullPath($SdkRoot)
    $ndkRoot = Join-Path $SdkRoot 'ndk/28.2.13676358'
    $toolchain = Join-Path $ndkRoot 'toolchains/llvm/prebuilt/windows-x86_64/bin'
    if (-not (Test-Path -LiteralPath $toolchain)) { throw "Install NDK 28.2.13676358 under $SdkRoot." }
    if (-not (Test-Path -LiteralPath (Join-Path $SdkRoot 'platforms/android-36/android.jar'))) {
        throw 'Install the Android SDK Platform 36.'
    }
    Assert-BuildBudget

    $target = 'aarch64-linux-android'
    $clangTarget = 'aarch64-linux-android'
    if ($Abi -eq 'x86_64') {
        $target = 'x86_64-linux-android'
        $clangTarget = 'x86_64-linux-android'
    }
    $targetKey = $target.Replace('-', '_')
    $cargoTargetKey = $targetKey.ToUpperInvariant()
    $clang = Join-Path $toolchain "${clangTarget}26-clang.cmd"
    $clangCxx = Join-Path $toolchain "${clangTarget}26-clang++.cmd"
    Set-BuildEnvironment 'CARGO_TARGET_DIR' (Join-Path $buildRoot 'cargo')
    Set-BuildEnvironment 'CARGO_BUILD_JOBS' '2'
    Set-BuildEnvironment 'CARGO_INCREMENTAL' '0'
    Set-BuildEnvironment 'CARGO_PROFILE_DEV_DEBUG' '0'
    Set-BuildEnvironment 'CARGO_PROFILE_RELEASE_DEBUG' '0'
    Set-BuildEnvironment "CARGO_TARGET_${cargoTargetKey}_LINKER" $clang
    Set-BuildEnvironment "CC_$targetKey" $clang
    Set-BuildEnvironment "CXX_$targetKey" $clangCxx
    Set-BuildEnvironment "AR_$targetKey" (Join-Path $toolchain 'llvm-ar.exe')
    Set-BuildEnvironment 'ANDROID_HOME' $SdkRoot
    Set-BuildEnvironment 'ANDROID_SDK_ROOT' $SdkRoot
    Set-BuildEnvironment 'ANDROID_NDK_HOME' $ndkRoot

    $cargoArgs = "build --locked --lib --target $target --jobs 2"
    $profile = 'debug'
    if ($Configuration -eq 'Release') {
        $cargoArgs += ' --release'
        $profile = 'release'
    }
    Write-Output "Building GPUI benchmark $Configuration for $Abi. Logs are in .build/logs."
    Invoke-BuildProcess (Get-Command cargo.exe).Source $cargoArgs 'cargo'

    $nativeDir = Join-Path $buildRoot "jniLibs/$Abi"
    New-Item -ItemType Directory -Path $nativeDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $buildRoot "cargo/$target/$profile/libgpui_lab.so") -Destination $nativeDir -Force
    $cxxRuntime = Join-Path $ndkRoot "toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/$target/libc++_shared.so"
    if (Test-Path -LiteralPath $cxxRuntime) {
        Copy-Item -LiteralPath $cxxRuntime -Destination $nativeDir -Force
    }

    $gradleWrapper = Join-Path $repoRoot 'android/gradlew.bat'
    $gradleArgs = '/d /s /c ""{0}" --project-dir "{1}" --project-cache-dir "{2}" --no-daemon --max-workers=2 -PlabAbi={3} :app:assemble{4}"' -f `
        $gradleWrapper, (Join-Path $repoRoot 'android'), (Join-Path $buildRoot 'gradle-cache'), $Abi, $Configuration
    Invoke-BuildProcess $env:ComSpec $gradleArgs 'gradle'

    $apk = Join-Path $buildRoot "gradle/app/outputs/apk/$profile/app-$profile.apk"
    $distDir = Join-Path $repoRoot 'dist'
    New-Item -ItemType Directory -Path $distDir -Force | Out-Null
    $destination = Join-Path $distDir "gpui-benchmark-$Abi-$profile.apk"
    Copy-Item -LiteralPath $apk -Destination $destination -Force
    $digest = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$destination.sha256", "$digest  $([IO.Path]::GetFileName($destination))`n")
    Write-Output "APK: $destination"
    Write-Output "SHA256: $digest"
}
finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    if ($lock) { $lock.Dispose() }
}
