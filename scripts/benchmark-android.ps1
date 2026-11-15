[CmdletBinding()]
param(
    [string]$Serial,
    [switch]$Quick,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$benchmarkPython = Get-Command python -ErrorAction SilentlyContinue
if (-not $benchmarkPython) {
    throw '需要 Python 3.10 或更高版本，请安装 Python 并加入 PATH。'
}
$benchmarkArguments = @((Join-Path $PSScriptRoot 'benchmark_android.py'))
if ($Serial) { $benchmarkArguments += @('--serial', $Serial) }
if ($Quick) { $benchmarkArguments += '--quick' }
if ($OutputDirectory) { $benchmarkArguments += @('--output-directory', $OutputDirectory) }
& $benchmarkPython.Source @benchmarkArguments
exit $LASTEXITCODE
