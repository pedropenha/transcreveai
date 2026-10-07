<#
.SYNOPSIS
  Transcribes the A/B cuts of a real meeting with one or more models and
  reports the differences.

.DESCRIPTION
  Second half of the meeting-quality harness. The first half is the Rust test
  `meeting::live::ab_variants` (src-tauri/src/meeting/live/ab_variants.rs),
  which cuts a meeting's WAV blocks with the production VAD + segmenter and
  writes a manifest.json. This script feeds every cut to
  `transcreve-ai --transcribe-file --json` for each model, then writes
  results.json (one row per model x cut, cached so a re-run resumes) and
  report.md.

  The variants answer separate questions:
    exact  - today's pipeline (spans cut rent to the VAD verdict)
    padded - the cheap fix (+450 ms pre-roll/tail, like dictation)
    whole  - one call per sealed 60 s block (no slicing inside a block)
    full   - one call for the whole track (no slicing at all)

  Note: --transcribe-file runs the same batch path as dictation, so it uses
  the app's persisted `selected_language` and `custom_words`. Keep those
  fixed across a comparison run or the columns are not comparable.

.EXAMPLE
  # 1. generate the cuts (PowerShell, from the repo root)
  . .\scripts\windows-dev-env.ps1 -BypassJunction
  $env:MEETING_AB_DIR = "$env:APPDATA\br.com.creator4all.transcreve.ai\audio\meetings\<id>"
  $env:MEETING_AB_OUT = "D:\ab"
  $env:MEETING_AB_BLOCKS = "1-5"
  cargo test -p transcreve-ai --lib ab_variants -- --ignored --nocapture

  # 2. transcribe and compare
  .\scripts\meeting-ab.ps1 -Manifest D:\ab\manifest.json
#>
[CmdletBinding()]
param(
    # manifest.json written by the ab_variants test.
    [Parameter(Mandatory = $true)]
    [string]$Manifest,

    # Model ids as `--list-models` prints them. Omit to use every installed model.
    [string[]]$Models,

    # Variants to run. Omit to run every variant present in the manifest.
    [string[]]$Variants,

    # App binary. It MUST be a debug build: release sets
    # `windows_subsystem = "windows"` (main.rs), so a release binary has no
    # stdout and every `--json` result comes back empty.
    [string]$Exe = "D:\t\debug\transcreve-ai.exe",

    # Output directory for results.json / report.md (default: the manifest's).
    [string]$Out,

    # Stop after this many cuts per model x variant (smoke run).
    [int]$MaxCuts = 0,

    # Re-transcribe cuts already present in results.json.
    [switch]$Fresh
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Manifest)) { throw "manifest not found: $Manifest" }
if (-not (Test-Path $Exe)) { throw "app binary not found: $Exe" }

$manifestData = Get-Content $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $Out) { $Out = Split-Path -Parent (Resolve-Path $Manifest) }
if (-not (Test-Path $Out)) { New-Item -ItemType Directory -Path $Out | Out-Null }

$resultsPath = Join-Path $Out 'results.json'
$reportPath = Join-Path $Out 'report.md'

# ---------------------------------------------------------------- models ----

function Get-InstalledModels {
    param([string]$Exe)
    # `--list-models --json` pretty-prints, so the whole output is one JSON
    # document — never a single line to pick out.
    $parsed = (& $Exe --list-models --json) -join "`n" | ConvertFrom-Json
    $rows = $parsed
    if ($parsed.PSObject.Properties.Name -contains 'models') { $rows = $parsed.models }
    $out = @()
    foreach ($m in $rows) {
        $downloaded = $m.is_downloaded
        if ($null -eq $downloaded) { $downloaded = $m.isDownloaded }
        if ($downloaded) { $out += $m.id }
    }
    return $out
}

if (-not $Models -or $Models.Count -eq 0) {
    Write-Host "No -Models given; querying installed models..." -ForegroundColor Cyan
    $Models = Get-InstalledModels -Exe $Exe
    if (-not $Models -or $Models.Count -eq 0) {
        throw "no installed models found; pass -Models explicitly"
    }
}
Write-Host "Models:" -ForegroundColor Cyan
foreach ($m in $Models) { Write-Host "  $m" }

$cuts = $manifestData.cuts
if ($Variants -and $Variants.Count -gt 0) {
    $cuts = $cuts | Where-Object { $Variants -contains $_.variant }
}
$variantNames = $cuts | Select-Object -ExpandProperty variant -Unique | Sort-Object
Write-Host "Variants: $($variantNames -join ', ')" -ForegroundColor Cyan

# --------------------------------------------------------------- results ----

# Cache keyed by "model|file" so an interrupted run resumes.
$results = @{}
if ((Test-Path $resultsPath) -and (-not $Fresh)) {
    $prior = Get-Content $resultsPath -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($r in $prior) { $results["$($r.model)|$($r.file)"] = $r }
    Write-Host "Reusing $($results.Count) cached result(s) from results.json" -ForegroundColor DarkGray
}

function Save-Results {
    param([hashtable]$Results, [string]$Path)
    $rows = $Results.Values | Sort-Object model, variant, track, block, index
    # -Depth matters: the rows are flat, but ConvertTo-Json truncates at 2.
    $rows | ConvertTo-Json -Depth 5 | Out-File -FilePath $Path -Encoding utf8
}

$total = 0
foreach ($model in $Models) {
    foreach ($variant in $variantNames) {
        $set = $cuts | Where-Object { $_.variant -eq $variant } | Sort-Object track, block, index
        if ($MaxCuts -gt 0) { $set = $set | Select-Object -First $MaxCuts }
        $total += $set.Count
    }
}
Write-Host "$total transcription(s) to do (cached ones are skipped)`n" -ForegroundColor Cyan

$done = 0
foreach ($model in $Models) {
    foreach ($variant in $variantNames) {
        $set = $cuts | Where-Object { $_.variant -eq $variant } | Sort-Object track, block, index
        if ($MaxCuts -gt 0) { $set = $set | Select-Object -First $MaxCuts }

        foreach ($cut in $set) {
            $done++
            $key = "$model|$($cut.file)"
            if ($results.ContainsKey($key)) { continue }
            if (-not (Test-Path $cut.file)) {
                Write-Warning "missing cut: $($cut.file)"
                continue
            }

            $shortModel = ($model -split '/')[-1]
            Write-Progress -Activity "meeting-ab" `
                -Status "$shortModel / $variant / $(Split-Path -Leaf $cut.file)" `
                -PercentComplete ([math]::Min(100, (100.0 * $done / [math]::Max(1, $total))))

            $raw = & $Exe --transcribe-file $cut.file --model $model --json
            if ($LASTEXITCODE -ne 0) {
                Write-Warning "transcribe failed (exit $LASTEXITCODE): $($cut.file)"
                continue
            }
            # `--transcribe-file --json` prints ONE compact object on its own
            # line; require both braces so a log line that merely opens with
            # '{' cannot be mistaken for the result.
            $jsonLine = ($raw | Where-Object {
                    $t = $_.Trim(); $t.StartsWith('{') -and $t.EndsWith('}')
                } | Select-Object -Last 1)
            if (-not $jsonLine) {
                Write-Warning "no JSON from: $($cut.file)"
                continue
            }
            $parsed = $jsonLine | ConvertFrom-Json

            $results[$key] = [pscustomobject]@{
                model      = $model
                variant    = $cut.variant
                track      = $cut.track
                block      = $cut.block
                index      = $cut.index
                start_ms   = $cut.start_ms
                end_ms     = $cut.end_ms
                file       = $cut.file
                audio_secs = $parsed.audio_secs
                best_ms    = $parsed.best_ms
                backend    = $parsed.bound_backend
                text       = $parsed.text
            }

            if ($done % 20 -eq 0) { Save-Results -Results $results -Path $resultsPath }
        }
    }
}
Write-Progress -Activity "meeting-ab" -Completed
Save-Results -Results $results -Path $resultsPath
Write-Host "results -> $resultsPath" -ForegroundColor Green

# --------------------------------------------------------------- metrics ----

function Get-Words {
    param([string]$Text)
    if (-not $Text) { return @() }
    $clean = $Text.ToLowerInvariant()
    # Keep letters (incl. accented), digits and spaces; punctuation is noise
    # for a word-level comparison.
    $clean = [regex]::Replace($clean, "[^\p{L}\p{Nd}\s]", ' ')
    return @([regex]::Split($clean.Trim(), '\s+') | Where-Object { $_ -ne '' })
}

# Word-level edit distance. Cuts are <= 30 s (~100 words), so the full DP is
# cheap; `full` cuts are excluded from pairwise comparison by the caller.
function Get-WordDistance {
    param([string[]]$A, [string[]]$B)
    $n = $A.Count
    $m = $B.Count
    if ($n -eq 0) { return $m }
    if ($m -eq 0) { return $n }
    $prev = New-Object 'int[]' ($m + 1)
    $cur = New-Object 'int[]' ($m + 1)
    for ($j = 0; $j -le $m; $j++) { $prev[$j] = $j }
    for ($i = 1; $i -le $n; $i++) {
        $cur[0] = $i
        for ($j = 1; $j -le $m; $j++) {
            $cost = 1
            if ($A[$i - 1] -eq $B[$j - 1]) { $cost = 0 }
            $del = $prev[$j] + 1
            $ins = $cur[$j - 1] + 1
            $sub = $prev[$j - 1] + $cost
            $best = $del
            if ($ins -lt $best) { $best = $ins }
            if ($sub -lt $best) { $best = $sub }
            $cur[$j] = $best
        }
        [array]::Copy($cur, $prev, $m + 1)
    }
    return $prev[$m]
}

$rows = $results.Values

$summary = @()
foreach ($model in $Models) {
    foreach ($variant in $variantNames) {
        $set = @($rows | Where-Object { $_.model -eq $model -and $_.variant -eq $variant })
        if ($set.Count -eq 0) { continue }
        $words = 0
        foreach ($r in $set) { $words += (Get-Words -Text $r.text).Count }
        $audio = ($set | Measure-Object -Property audio_secs -Sum).Sum
        $ms = ($set | Measure-Object -Property best_ms -Sum).Sum
        $empty = @($set | Where-Object { -not $_.text -or $_.text.Trim() -eq '' }).Count
        $rtf = 0
        if ($ms -gt 0) { $rtf = [math]::Round($audio / ($ms / 1000.0), 2) }
        $summary += [pscustomobject]@{
            Model       = ($model -split '/')[-1]
            Variant     = $variant
            Cuts        = $set.Count
            AudioSecs   = [math]::Round($audio, 1)
            Words       = $words
            EmptyCuts   = $empty
            TotalSecs   = [math]::Round($ms / 1000.0, 1)
            RTFx        = $rtf
            FullModelId = $model
        }
    }
}

# exact vs padded, cut by cut: does the 450 ms pre-roll/tail change the text,
# and does it change specifically the FIRST/LAST word (the clipping claim)?
$edgeStats = @()
foreach ($model in $Models) {
    $exactRows = @($rows | Where-Object { $_.model -eq $model -and $_.variant -eq 'exact' })
    if ($exactRows.Count -eq 0) { continue }
    $pairs = 0; $changed = 0; $firstDiff = 0; $lastDiff = 0
    $distSum = 0; $refSum = 0; $exactWords = 0; $paddedWords = 0
    foreach ($e in $exactRows) {
        $p = $rows | Where-Object {
            $_.model -eq $model -and $_.variant -eq 'padded' -and
            $_.track -eq $e.track -and $_.block -eq $e.block -and $_.index -eq $e.index
        } | Select-Object -First 1
        if (-not $p) { continue }
        $pairs++
        $ew = Get-Words -Text $e.text
        $pw = Get-Words -Text $p.text
        $exactWords += $ew.Count
        $paddedWords += $pw.Count
        $d = Get-WordDistance -A $ew -B $pw
        $distSum += $d
        $refSum += $pw.Count
        if ($d -gt 0) { $changed++ }
        $ef = ''; $pf = ''; $el = ''; $pl = ''
        if ($ew.Count -gt 0) { $ef = $ew[0]; $el = $ew[$ew.Count - 1] }
        if ($pw.Count -gt 0) { $pf = $pw[0]; $pl = $pw[$pw.Count - 1] }
        if ($ef -ne $pf) { $firstDiff++ }
        if ($el -ne $pl) { $lastDiff++ }
    }
    if ($pairs -eq 0) { continue }
    $divergence = 0
    if ($refSum -gt 0) { $divergence = [math]::Round(100.0 * $distSum / $refSum, 1) }
    $edgeStats += [pscustomobject]@{
        Model            = ($model -split '/')[-1]
        Pairs            = $pairs
        ChangedPct       = [math]::Round(100.0 * $changed / $pairs, 1)
        FirstWordDiffPct = [math]::Round(100.0 * $firstDiff / $pairs, 1)
        LastWordDiffPct  = [math]::Round(100.0 * $lastDiff / $pairs, 1)
        DivergencePct    = $divergence
        WordsExact       = $exactWords
        WordsPadded      = $paddedWords
    }
}

# ---------------------------------------------------------------- report ----

function Format-MarkdownTable {
    param([object[]]$Rows, [string[]]$Columns)
    if (-not $Rows -or $Rows.Count -eq 0) { return "_(sem dados)_`n" }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('| ' + ($Columns -join ' | ') + ' |')
    [void]$sb.AppendLine('|' + (($Columns | ForEach-Object { '---' }) -join '|') + '|')
    foreach ($r in $Rows) {
        $cells = foreach ($c in $Columns) { [string]$r.$c }
        [void]$sb.AppendLine('| ' + ($cells -join ' | ') + ' |')
    }
    [void]$sb.AppendLine()
    return $sb.ToString()
}

function Join-Text {
    param([object[]]$Rows)
    $ordered = $Rows | Sort-Object track, block, index
    $parts = foreach ($r in $ordered) { $r.text.Trim() }
    return (($parts | Where-Object { $_ -ne '' }) -join ' ')
}

$md = New-Object System.Text.StringBuilder
[void]$md.AppendLine('# Reuniao A/B - modelo vs. fatiamento')
[void]$md.AppendLine()
[void]$md.AppendLine("- Gerado: $(Get-Date -Format 'yyyy-MM-dd HH:mm')")
[void]$md.AppendLine("- Audio: ``$($manifestData.meeting_dir)``")
[void]$md.AppendLine("- Padding da variante ``padded``: $($manifestData.pad_ms) ms por lado")
[void]$md.AppendLine("- Binario: ``$Exe``")
[void]$md.AppendLine()
[void]$md.AppendLine('As variantes: `exact` = pipeline de hoje (corte rente ao VAD);')
[void]$md.AppendLine('`padded` = correcao barata (+pre-roll/cauda, como o ditado);')
[void]$md.AppendLine('`whole` = um bloco de 60 s por chamada; `full` = a trilha inteira numa chamada.')
[void]$md.AppendLine()
[void]$md.AppendLine('## Resumo por modelo x variante')
[void]$md.AppendLine()
[void]$md.Append((Format-MarkdownTable -Rows $summary -Columns @(
    'Model', 'Variant', 'Cuts', 'AudioSecs', 'Words', 'EmptyCuts', 'TotalSecs', 'RTFx')))
[void]$md.AppendLine('`Words` e um proxy, nao um acerto: mais palavras pode ser mais fala')
[void]$md.AppendLine('recuperada ou mais alucinacao. `EmptyCuts` = cortes que nao produziram texto.')
[void]$md.AppendLine()
[void]$md.AppendLine('## `exact` vs `padded`, corte por corte')
[void]$md.AppendLine()
[void]$md.Append((Format-MarkdownTable -Rows $edgeStats -Columns @(
    'Model', 'Pairs', 'ChangedPct', 'FirstWordDiffPct', 'LastWordDiffPct', 'DivergencePct',
    'WordsExact', 'WordsPadded')))
[void]$md.AppendLine('`FirstWordDiffPct` / `LastWordDiffPct` sao o teste direto da hipotese de')
[void]$md.AppendLine('amputacao: se o pre-roll/cauda importa, a diferenca concentra-se nas bordas.')
[void]$md.AppendLine('`DivergencePct` = distancia de edicao por palavra entre as duas versoes.')
[void]$md.AppendLine()
[void]$md.AppendLine('## Transcricoes')
[void]$md.AppendLine()
foreach ($model in $Models) {
    foreach ($variant in $variantNames) {
        $set = @($rows | Where-Object { $_.model -eq $model -and $_.variant -eq $variant })
        if ($set.Count -eq 0) { continue }
        [void]$md.AppendLine("### $(($model -split '/')[-1]) - $variant")
        [void]$md.AppendLine()
        [void]$md.AppendLine((Join-Text -Rows $set))
        [void]$md.AppendLine()
    }
}

$md.ToString() | Out-File -FilePath $reportPath -Encoding utf8
Write-Host "report  -> $reportPath" -ForegroundColor Green
$summary | Format-Table Model, Variant, Cuts, AudioSecs, Words, EmptyCuts, TotalSecs, RTFx
if ($edgeStats.Count -gt 0) { $edgeStats | Format-Table * }
