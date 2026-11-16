[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ClientManifest.psm1') -Force
$sfSource = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/snowflakes-client/snowflakes.manifest'
$sfXml = [IO.File]::ReadAllText($sfSource)
$sfAssertions = 0
function Accept([string]$Xml) {
    Assert-SnowManifest -Xml $Xml
    $script:sfAssertions++
}
function Reject([string]$Xml) {
    $sfRejected = $false
    try { Assert-SnowManifest -Xml $Xml }
    catch { $sfRejected = $_.Exception.Message -eq 'Client manifest policy rejected' }
    if (-not $sfRejected) { throw 'Invalid manifest was accepted' }
    $script:sfAssertions++
}
Accept $sfXml
Accept $sfXml.Replace('>PerMonitorV2<','> PerMonitorV2 <')
foreach ($sfPair in @(
    @('asInvoker','requireAdministrator'), @('asInvoker','highestAvailable'),
    @('uiAccess="false"','uiAccess="true"'), @('PerMonitorV2','System'),
    @('true/pm','true'), @('amd64','x86'), @('Snowflakes.Client','Other.Client'),
    @('version="0.1.0.0"','version="not-a-version"'),
    @('{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}','{00000000-0000-0000-0000-000000000000}'),
    @('http://schemas.microsoft.com/SMI/2016/WindowsSettings','urn:wrong'),
    @('urn:schemas-microsoft-com:compatibility.v1','urn:wrong')
)) { Reject $sfXml.Replace($sfPair[0],$sfPair[1]) }
Reject ($sfXml -replace '<dpiAwareness[^>]*>.*?</dpiAwareness>','')
Reject $sfXml.Replace('</windowsSettings>','<autoElevate>true</autoElevate></windowsSettings>')
Reject $sfXml.Replace('</compatibility>','<maxversiontested Id="10.0.19045.0" /></compatibility>')
Reject $sfXml.Replace('</requestedPrivileges>','<requestedExecutionLevel level="asInvoker" uiAccess="false" /></requestedPrivileges>')
Reject $sfXml.Replace('</assembly>','')
Reject ($sfXml -replace '<\?xml[^>]*\?>','<!DOCTYPE assembly [<!ENTITY content "text">]>')
Reject '<assembly manifestVersion="1.0" />'
Write-Output "Client manifest policy: $sfAssertions assertions passed"
