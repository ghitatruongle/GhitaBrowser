# Synthetic media fixtures

These files contain no third-party video, imagery, music or website code. They
were generated locally from FFmpeg's solid-color and sine-wave sources solely
for deterministic decoder/MSE tests. FFmpeg is a development-time generator;
its executable and libraries are not copied into GhitaBrowser.

The complete-file fixture is a 64x64 solid blue H.264 Baseline video with a
440 Hz AAC audio track. The `mse` fixtures are a 64x64 solid green H.264
Baseline track and a 660 Hz AAC track split into initialization and media
segments.

SHA-256:

```text
33E3B27878B3B0F120C49D6400259EFE3635364A1BD36974DB4E3E1F430B1DC2  clear-avc-aac.mp4
6B8EF3D3B531067C85FA75EBB7AE31112DC8D045531866F39A1FCAFFABB13EE5  mse/audio-1.m4s
103B6F4000ED404A1EE6A46088599A88C7C69002F0CBCA25DDE19201850A904A  mse/audio-2.m4s
0A3B98BF50E2BDF15B1DCFC06E07623392799BA04A12D085840A187FEFB66C5D  mse/audio-init.mp4
96CC2E83956BD5549D13D037CF21AD24DAF8C3C9510F47B922C5C808026E8F3A  mse/video-1.m4s
4A3C66F76F5FEF5C009D55A168675082E89C507E1B7A78568008403CAD23DF77  mse/video-2.m4s
046927CBE56F50395740EB02D8E9F1FE9E193E0C82C3BE4BA260E829C1DD0BBA  mse/video-init.mp4
```
