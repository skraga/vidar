# RNNoise (vendored)

Source of [xiph/rnnoise](https://github.com/xiph/rnnoise) at commit
`70f1d256acd4b34a572f999a05c87bf00b67730d` (see `UPSTREAM_COMMIT`). BSD-3-Clause, see `COPYING`.

Only the files needed for the C build are kept: the portable core plus the x86 run-time
CPU dispatch (`src/x86/`, SSE4.1 and AVX2 network layers). No training code.

## Model weights

`model/rnnoise_data.c` (78 MB) is not in git. It comes from the tarball named in `model_version`:

```
https://media.xiph.org/rnnoise/models/rnnoise_data-<model_version>.tar.gz
```

The SHA-256 of the tarball equals the hash in `model_version`. `cargo xtask fetch` downloads it,
checks the hash and extracts `rnnoise_data.c` and `rnnoise_data.h` into `model/`.
