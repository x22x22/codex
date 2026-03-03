$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::InputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$script:CodexTmpDir = if ($env:CODEX_PS_TMP_DIR) {
    $env:CODEX_PS_TMP_DIR
} else {
    (Get-Location).Path
}
$script:ToolCounter = 0
$script:ActiveExecId = $null

function Send-KernelMessage {
    param(
        [Parameter(Mandatory)]
        [hashtable]$Message
    )

    $json = $Message | ConvertTo-Json -Compress -Depth 100
    [Console]::Out.WriteLine($json)
    [Console]::Out.Flush()
}

function Read-KernelMessage {
    while ($true) {
        $line = [Console]::In.ReadLine()
        if ($null -eq $line) {
            return $null
        }
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }

        try {
            return $line | ConvertFrom-Json -AsHashtable -Depth 100
        } catch {
            continue
        }
    }
}

function Format-StreamItem {
    param($Item)

    if ($null -eq $Item) {
        return $null
    }
    if ($Item -is [string]) {
        return $Item.TrimEnd("`r", "`n")
    }

    $text = $Item | Out-String -Width 4096
    $trimmed = $text.TrimEnd("`r", "`n")
    if ([string]::IsNullOrEmpty($trimmed)) {
        return $null
    }
    $trimmed
}

function Wait-ToolResult {
    param(
        [Parameter(Mandatory)]
        [string]$Id
    )

    while ($true) {
        $message = Read-KernelMessage
        if ($null -eq $message) {
            throw "ps_repl kernel closed while waiting for tool result"
        }
        if ($message.type -ne 'run_tool_result') {
            throw "ps_repl kernel received unexpected message while waiting for tool result: $($message.type)"
        }
        if ($message.id -ne $Id) {
            throw "ps_repl kernel received mismatched tool result: expected $Id, got $($message.id)"
        }
        return $message
    }
}

function Invoke-CodexTool {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [object]$Arguments
    )

    if ([string]::IsNullOrWhiteSpace($Name)) {
        throw "Invoke-CodexTool expects a non-empty tool name"
    }
    if ($null -eq $script:ActiveExecId) {
        throw "Invoke-CodexTool can only be used while a ps_repl exec is running"
    }

    $toolId = "{0}-tool-{1}" -f $script:ActiveExecId, $script:ToolCounter
    $script:ToolCounter += 1

    $argumentsJson = '{}'
    if ($PSBoundParameters.ContainsKey('Arguments')) {
        if ($Arguments -is [string]) {
            $argumentsJson = $Arguments
        } else {
            $argumentsJson = $Arguments | ConvertTo-Json -Compress -Depth 100
        }
    }

    Send-KernelMessage @{
        type      = 'run_tool'
        id        = $toolId
        exec_id   = $script:ActiveExecId
        tool_name = $Name
        arguments = $argumentsJson
    }

    $result = Wait-ToolResult -Id $toolId
    if (-not $result.ok) {
        if ($null -ne $result.error -and -not [string]::IsNullOrWhiteSpace([string]$result.error)) {
            throw [System.Exception]::new([string]$result.error)
        }
        throw [System.Exception]::new('tool failed')
    }

    $result.response
}

$script:Codex = [pscustomobject]@{
    TmpDir = $script:CodexTmpDir
}
$null = $script:Codex | Add-Member -MemberType ScriptMethod -Name Tool -Value {
    param($Name, $Arguments)

    if ($PSBoundParameters.ContainsKey('Arguments')) {
        Invoke-CodexTool -Name $Name -Arguments $Arguments
    } else {
        Invoke-CodexTool -Name $Name
    }
}
Set-Variable -Name Codex -Scope Script -Value $script:Codex
Set-Variable -Name CodexTmpDir -Scope Script -Value $script:CodexTmpDir

while ($true) {
    $message = Read-KernelMessage
    if ($null -eq $message) {
        break
    }
    if ($message.type -ne 'exec') {
        continue
    }

    $script:ActiveExecId = [string]$message.id

    try {
        $scriptBlock = [scriptblock]::Create([string]$message.code)
        $items = @(. $scriptBlock *>&1)
        $outputLines = foreach ($item in $items) {
            $formatted = Format-StreamItem -Item $item
            if ($null -ne $formatted -and $formatted -ne '') {
                $formatted
            }
        }
        $output = [string]::Join("`n", @($outputLines))
        Send-KernelMessage @{
            type   = 'exec_result'
            id     = [string]$message.id
            ok     = $true
            output = $output
            error  = $null
        }
    } catch {
        $errorMessage = if ($_.Exception -and $_.Exception.Message) {
            [string]$_.Exception.Message
        } else {
            [string]$_
        }
        Send-KernelMessage @{
            type   = 'exec_result'
            id     = [string]$message.id
            ok     = $false
            output = ''
            error  = $errorMessage
        }
    } finally {
        $script:ActiveExecId = $null
    }
}
