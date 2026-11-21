#!/usr/bin/env bash
# Generates test media into tests/fixtures. Requires ffmpeg with libx264, libx265, libvpx-vp9, libsvtav1, libwebp.
set -euo pipefail

out="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures"
mkdir -p "$out"
ff=(ffmpeg -hide_banner -loglevel error -y)

# Red disc with a soft alpha edge (straight alpha), optionally moving, on a transparent canvas.
disc() { # width height fps duration
  echo "color=c=black@0:s=${1}x${2}:r=${3}:d=${4},format=rgba,geq=r=255:g='60+150*X/W':b=60:a='255*clip((0.42*min(W\,H)-hypot(X-W/2-0.1*W*sin(2*PI*T/${4})\,Y-H/2))/6\,0\,1)'"
}

"${ff[@]}" -f lavfi -i "$(disc 256 256 1 1)" -frames:v 1 "$out/alpha.png"

# 1-pixel stripes: only pixel-exact output keeps full contrast (sharpness check).
"${ff[@]}" -f lavfi -i "color=black:s=320x320,format=rgba,geq=r='255*lt(mod(X,2),1)':g='255*lt(mod(X,2),1)':b='255*lt(mod(X,2),1)':a=255" \
  -frames:v 1 "$out/stripes320.png"

# Four opaque asymmetric quadrants used to prove rotation and mirror direction.
quad="color=c=black:s=200x100,drawbox=x=0:y=0:w=100:h=50:c=red:t=fill,drawbox=x=100:y=0:w=100:h=50:c=green:t=fill,drawbox=x=0:y=50:w=100:h=50:c=yellow:t=fill,drawbox=x=100:y=50:w=100:h=50:c=magenta:t=fill"
"${ff[@]}" -f lavfi -i "$quad" -frames:v 1 "$out/asymmetric.png"
"${ff[@]}" -f lavfi -i "$quad" -frames:v 1 -loop 0 "$out/asymmetric.gif"

# Same stripes at 400 px: shown at 320 logical px on a 1.25 output it maps 1:1 to physical pixels.
"${ff[@]}" -f lavfi -i "color=black:s=400x400,format=rgba,geq=r='255*lt(mod(X,2),1)':g='255*lt(mod(X,2),1)':b='255*lt(mod(X,2),1)':a=255" \
  -frames:v 1 "$out/stripes400.png"

"${ff[@]}" -f lavfi -i "$(disc 200 200 20 2)" \
  -filter_complex "[0:v]split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse=alpha_threshold=128" \
  -loop 0 "$out/anim.gif"

"${ff[@]}" -f lavfi -i "$(disc 200 200 20 2)" -plays 0 -f apng "$out/anim.apng"

"${ff[@]}" -f lavfi -i "$(disc 200 200 20 2)" -c:v libwebp_anim -lossless 1 -loop 0 "$out/anim.webp"

"${ff[@]}" -f lavfi -i "$(disc 320 240 30 3)" -c:v libvpx-vp9 -pix_fmt yuva420p -auto-alt-ref 0 -b:v 1M "$out/alpha.webm"

"${ff[@]}" -f lavfi -i "testsrc2=s=1920x1080:r=30:d=10" -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$out/h264_1080p.mp4"

# Burned-in timestamp so loops and playback progress are visible in screenshots.
font="$(fc-match -f '%{file}' monospace 2>/dev/null || echo /usr/share/fonts/TTF/DejaVuSansMono.ttf)"
stamp="drawtext=fontfile=${font}:text='%{pts\\:hms}':fontsize=420:fontcolor=white:box=1:boxcolor=black@0.7:x=(w-tw)/2:y=(h-th)/2"
"${ff[@]}" -f lavfi -i "testsrc2=s=3840x2160:r=30:d=60" -vf "$stamp" -c:v libx265 -preset ultrafast -crf 30 -pix_fmt yuv420p \
  -x265-params log-level=error "$out/hevc_4k_60s.mp4"

"${ff[@]}" -f lavfi -i "testsrc2=s=1920x1080:r=30:d=10" -c:v libsvtav1 -preset 10 -crf 40 -pix_fmt yuv420p "$out/av1_1080p.mp4" 2>/dev/null

"${ff[@]}" -f lavfi -i "testsrc2=s=320x568:r=30:d=5" -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$out/vertical.mp4"
"${ff[@]}" -f lavfi -i "testsrc2=s=320x568:r=30:d=15" -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p "$out/vertical15.mp4"

# Big 60 fps alpha animation (the real use case): 5 s of 1300x1300, upscaled from a small render to keep generation fast.
"${ff[@]}" -f lavfi -i "$(disc 260 260 60 5),scale=1300:1300:flags=bicubic,format=rgba" -plays 0 -f apng "$out/big_alpha.apng"

# Undecodable media: the extension picks a demuxer, the bytes are noise.
head -c 4096 /dev/urandom >"$out/corrupt.png"

ls -lh "$out"
