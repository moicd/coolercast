# Renders assets/coolercast.ico from assets/logo.png (the full logo: a dark rounded badge with a
# microphone around a fan and the "COOLERCAST" wordmark). The wordmark is unreadable below 128
# pixels, so smaller sizes show only the microphone, zoomed in on the same badge.
# Run with Windows PowerShell 5.1.

Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 80, 96, 128, 256
$logo = [System.Drawing.Bitmap]::FromFile((Join-Path $PSScriptRoot 'logo.png'))
$out = Join-Path $PSScriptRoot 'coolercast.ico'

# The microphone in logo.png, as fractions of its size: square in the original artwork.
$mark = New-Object System.Drawing.RectangleF (0.154 * $logo.Width), (0.063 * $logo.Height), (0.689 * $logo.Width), (0.670 * $logo.Height)

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $path
}

function Render([int]$size) {
    $source = if ($size -ge 128) {
        New-Object System.Drawing.RectangleF 0, 0, $logo.Width, $logo.Height
    } else {
        $mark
    }

    # Scale the artwork first, then cut the badge out of it with an anti-aliased fill.
    $art = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($art)
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $wrap = New-Object System.Drawing.Imaging.ImageAttributes
    $wrap.SetWrapMode('TileFlipXY')
    $g.DrawImage($logo, (New-Object System.Drawing.Rectangle 0, 0, $size, $size),
        $source.X, $source.Y, $source.Width, $source.Height, 'Pixel', $wrap)
    $g.Dispose()

    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $inset = if ($size -ge 32) { 0.5 } else { 0 }
    $badge = New-RoundedRect $inset $inset ($size - 2 * $inset) ($size - 2 * $inset) ($size * 0.22)
    $g.FillPath((New-Object System.Drawing.TextureBrush $art), $badge)
    # A faint rim keeps the dark badge visible on a dark taskbar.
    if ($size -ge 32) {
        $rim = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(48, 255, 255, 255)), 1
        $g.DrawPath($rim, $badge)
    }
    $g.Dispose()
    $art.Dispose()

    $ms = New-Object System.IO.MemoryStream
    if ($size -ge 64) {
        # Large sizes are stored as PNG.
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    } else {
        # Small sizes as a classic 32-bit DIB: header, bottom-up BGRA rows, then the AND mask.
        $bw = New-Object System.IO.BinaryWriter $ms
        $bw.Write([uint32]40); $bw.Write([int32]$size); $bw.Write([int32]($size * 2))
        $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]0)
        $bw.Write([uint32]($size * $size * 4)); $bw.Write([int32]0); $bw.Write([int32]0)
        $bw.Write([uint32]0); $bw.Write([uint32]0)
        for ($y = $size - 1; $y -ge 0; $y--) {
            for ($x = 0; $x -lt $size; $x++) {
                $c = $bmp.GetPixel($x, $y)
                $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
            }
        }
        $maskStride = [int]([Math]::Ceiling($size / 32.0) * 4)
        for ($y = $size - 1; $y -ge 0; $y--) {
            $row = New-Object byte[] $maskStride
            for ($x = 0; $x -lt $size; $x++) {
                if ($bmp.GetPixel($x, $y).A -eq 0) { $row[[int][Math]::Floor($x / 8)] = $row[[int][Math]::Floor($x / 8)] -bor (0x80 -shr ($x % 8)) }
            }
            $bw.Write($row)
        }
        $bw.Flush()
    }
    $bmp.Dispose()
    , $ms.ToArray()
}

$images = foreach ($s in $sizes) { , (Render $s) }
$logo.Dispose()

$fs = [System.IO.File]::Create($out)
$w = New-Object System.IO.BinaryWriter $fs
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $data = $images[$i]
    $dim = if ($s -ge 256) { 0 } else { $s }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$data.Length); $w.Write([uint32]$offset)
    $offset += $data.Length
}
foreach ($data in $images) { $w.Write($data) }
$w.Close()
Write-Output "wrote $out ($((Get-Item $out).Length) bytes)"
