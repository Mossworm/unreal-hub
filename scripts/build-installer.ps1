$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'Cargo.toml'
$versionMatch = Select-String -Path $manifest -Pattern '^version = "(.+)"' | Select-Object -First 1
if (-not $versionMatch) {
    throw 'Cargo.toml에서 버전을 찾을 수 없습니다.'
}
$version = $versionMatch.Matches[0].Groups[1].Value

$compilerCandidates = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
)
$compiler = $compilerCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $compiler) {
    throw 'Inno Setup 6(ISCC.exe)을 찾을 수 없습니다. winget install JRSoftware.InnoSetup 으로 설치하세요.'
}

cargo build --release --manifest-path $manifest
if ($LASTEXITCODE -ne 0) {
    throw "cargo build 실패 (종료 코드 $LASTEXITCODE)"
}

& $compiler "/DAppVersion=$version" (Join-Path $root 'installer\unreal-hub.iss')
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup 컴파일 실패 (종료 코드 $LASTEXITCODE)"
}

$installer = Join-Path $root "dist\UnrealHubSetup.exe"
$hash = (Get-FileHash -Algorithm SHA256 $installer).Hash.ToLowerInvariant()
Write-Host "설치 파일: $installer"
Write-Host "SHA-256: $hash"
