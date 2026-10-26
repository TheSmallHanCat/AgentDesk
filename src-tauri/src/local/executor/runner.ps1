$ErrorActionPreference = 'Stop'
$PSStyle.OutputRendering = 'PlainText'
$utf8 = [Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = $utf8
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8
$global:LASTEXITCODE = 0
try {
    $source = [Console]::In.ReadToEnd()
    $script = [ScriptBlock]::Create($source)
    & $script
    exit $global:LASTEXITCODE
} catch {
    [Console]::Error.WriteLine(($_ | Out-String).TrimEnd())
    exit 1
}
