Set-StrictMode -Version Latest

function Get-SnowManifestTool {
    $sfCommand = Get-Command mt.exe -ErrorAction SilentlyContinue
    if ($sfCommand) { return $sfCommand.Source }
    $sfBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    if (Test-Path -LiteralPath $sfBin) {
        foreach ($sfVersion in Get-ChildItem -LiteralPath $sfBin -Directory |
            Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
            Sort-Object { [version]$_.Name } -Descending) {
            $sfTool = Join-Path $sfVersion.FullName 'x64/mt.exe'
            if (Test-Path -LiteralPath $sfTool) { return $sfTool }
        }
    }
    throw 'Client manifest check requires the Windows SDK manifest tool'
}

function Assert-SnowManifest {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Xml)
    try {
        $sfSettings = [Xml.XmlReaderSettings]::new()
        $sfSettings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
        $sfSettings.XmlResolver = $null
        $sfString = [IO.StringReader]::new($Xml)
        $sfReader = [Xml.XmlReader]::Create($sfString, $sfSettings)
        try {
            $sfDocument = [Xml.XmlDocument]::new()
            $sfDocument.XmlResolver = $null
            $sfDocument.Load($sfReader)
        } finally { $sfReader.Dispose(); $sfString.Dispose() }
        $sfNs = [Xml.XmlNamespaceManager]::new($sfDocument.NameTable)
        foreach ($sfPair in @(
            @('a','urn:schemas-microsoft-com:asm.v1'),
            @('v','urn:schemas-microsoft-com:asm.v3'),
            @('c','urn:schemas-microsoft-com:compatibility.v1'),
            @('d','http://schemas.microsoft.com/SMI/2005/WindowsSettings'),
            @('p','http://schemas.microsoft.com/SMI/2016/WindowsSettings')
        )) { $sfNs.AddNamespace($sfPair[0],$sfPair[1]) }
        function One([string]$Path) {
            $sfNodes = $sfDocument.SelectNodes($Path,$sfNs)
            if ($sfNodes.Count -ne 1) { throw 'Expected one manifest element' }
            return $sfNodes[0]
        }
        $sfAssembly = One '/a:assembly'
        $sfIdentity = One '/a:assembly/a:assemblyIdentity'
        $sfLevel = One '/a:assembly/v:trustInfo/v:security/v:requestedPrivileges/v:requestedExecutionLevel'
        $sfOs = One '/a:assembly/c:compatibility/c:application/c:supportedOS'
        $sfDpi = One '/a:assembly/v:application/v:windowsSettings/p:dpiAwareness'
        $sfLegacy = One '/a:assembly/v:application/v:windowsSettings/d:dpiAware'
        if ($sfAssembly.GetAttribute('manifestVersion') -ne '1.0' -or
            $sfIdentity.GetAttribute('name') -ne 'Snowflakes.Client' -or
            $sfIdentity.GetAttribute('type') -ne 'win32' -or
            $sfIdentity.GetAttribute('processorArchitecture') -ne 'amd64' -or
            $sfIdentity.GetAttribute('version') -notmatch '^\d+\.\d+\.\d+\.\d+$' -or
            $sfLevel.GetAttribute('level') -ne 'asInvoker' -or
            $sfLevel.GetAttribute('uiAccess') -ne 'false' -or
            $sfOs.GetAttribute('Id') -ne '{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}' -or
            $sfDpi.InnerText.Trim() -ne 'PerMonitorV2' -or $sfLegacy.InnerText.Trim() -ne 'true/pm' -or
            $sfDocument.SelectNodes('//*[local-name()="autoElevate" or local-name()="maxversiontested"]').Count -ne 0) {
            throw 'Unexpected manifest policy'
        }
    } catch { throw 'Client manifest policy rejected' }
}

function Get-SnowEmbeddedManifest {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Binary)
    $sfTool = Get-SnowManifestTool
    $sfFile = Join-Path ([IO.Path]::GetTempPath()) ('snowflakes-manifest-' + [guid]::NewGuid().ToString('N') + '.xml')
    try {
        $null = & $sfTool "-inputresource:$([IO.Path]::GetFullPath($Binary));#1" "-out:$sfFile" 2>&1
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $sfFile)) {
            throw 'Resource missing'
        }
        $sfXml = [IO.File]::ReadAllText($sfFile)
        Assert-SnowManifest -Xml $sfXml
        $null = & $sfTool -validate_manifest -manifest $sfFile 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Schema validation failed' }
        return $sfXml
    } catch { throw 'Client manifest resource or policy is unavailable' }
    finally { if (Test-Path -LiteralPath $sfFile) { Remove-Item -LiteralPath $sfFile } }
}

Export-ModuleMember -Function Assert-SnowManifest, Get-SnowEmbeddedManifest
