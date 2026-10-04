# Renders assets/coolercast.ico: a teal rounded square with a white "°C" mark.
# Each size is drawn separately so small sizes stay crisp. Run with Windows PowerShell 5.1.

Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$out = Join-Path $PSScriptRoot 'coolercast.ico'

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
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)

    # Badge
    $inset = [Math]::Max(0.5, $size / 32)
    $rect = New-Object System.Drawing.RectangleF $inset, $inset, ($size - 2 * $inset), ($size - 2 * $inset)
    $badge = New-RoundedRect $rect.X $rect.Y $rect.Width $rect.Height ($size * 0.22)
    $top = [System.Drawing.Color]::FromArgb(255, 20, 184, 166)     # #14B8A6
    $bottom = [System.Drawing.Color]::FromArgb(255, 15, 118, 110)  # #0F766E
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, $top, $bottom, 90
    $g.FillPath($brush, $badge)

    # "C": a thick arc opening to the right
    $stroke = [Math]::Max(2, $size * 0.13)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), $stroke
    $pen.StartCap = 'Round'
    $pen.EndCap = 'Round'
    $cx = $size * 0.56; $cy = $size * 0.55; $r = $size * 0.25
    $g.DrawArc($pen, $cx - $r, $cy - $r, 2 * $r, 2 * $r, 45, 270)

    # Degree ring, top left
    $ringStroke = [Math]::Max(1.5, $size * 0.07)
    $ringPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), $ringStroke
    $dr = [Math]::Max(1.6, $size * 0.085)
    $dx = $size * 0.24; $dy = $size * 0.27
    $g.DrawEllipse($ringPen, $dx - $dr, $dy - $dr, 2 * $dr, 2 * $dr)

    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    if ($size -ge 256) {
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
