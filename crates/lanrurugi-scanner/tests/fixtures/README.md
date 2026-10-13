# Animation fixtures

`animated_5_frames.jxl` — the five-frame "numbered frames" sample shipped in the `jxl` crate's own
`tests/testdata/` (the pure-Rust JPEG XL decoder, BSD-3-Clause, the implementation Chrome and
Firefox use). Synthetic content; used by `src/animation.rs`'s animated-JXL test so the conversion
path can be verified without a real-world JXL animation, which the library does not have.

## `still_2x2.avif`, `still_4x4.jp2`

Synthetic single-colour images generated locally with `ffmpeg`: the AVIF with
`color=c=red:s=2x2 -c:v libaom-av1 -f avif`, the JPEG 2000 with
`color=c=gray:s=4x4 -pix_fmt gray -c:v jpeg2000` (grayscale on purpose — a colour JP2 from ffmpeg
uses the sYCC colour space, which `jpeg2k` refuses to convert). They prove the `avif-decode`
(`rav1d`) and `jpeg2k` (OpenJPEG) paths are actually wired in. No third-party content.
