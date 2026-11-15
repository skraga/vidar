//! Compiles xiph/rnnoise (vendored in third_party/rnnoise, commit in UPSTREAM_COMMIT)
//! as plain C.
//!
//! The model weights (rnnoise_data.c, 78 MB) are not in git. Fetch them with
//! `cargo xtask fetch`, or point VIDAR_RNNOISE_DATA_DIR at a folder holding
//! rnnoise_data.c and rnnoise_data.h.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let rn = manifest.join("../../third_party/rnnoise");
    let data_dir = std::env::var_os("VIDAR_RNNOISE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| rn.join("model"));

    let data_c = data_dir.join("rnnoise_data.c");
    if !data_c.is_file() {
        panic!(
            "RNNoise model data not found at {}.\nRun `cargo xtask fetch` from the repository root first.",
            data_c.display()
        );
    }

    let src = rn.join("src");
    let mut build = cc::Build::new();
    build
        .include(rn.join("include"))
        .include(&src)
        .include(&data_dir)
        .define("RNNOISE_BUILD", None)
        .warnings(false);
    for f in [
        "celt_lpc.c",
        "denoise.c",
        "kiss_fft.c",
        "parse_lpcnet_weights.c",
        "pitch.c",
        "rnn.c",
        "rnnoise_tables.c",
        "nnet.c",
        "nnet_default.c",
    ] {
        build.file(src.join(f));
    }
    build.file(&data_c);
    // The weights file is huge; optimise it even in debug builds or compile takes minutes.
    build.opt_level(2);

    // x86-64: upstream's run-time CPU dispatch. The network layers get SSE4.1 and AVX2
    // versions, each compiled with its own instruction set, and the fastest one the CPU
    // supports is picked when a denoiser is created. The rest stays baseline x86-64, so the
    // binary still runs on any 64-bit CPU.
    let x86_64 = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64");
    if x86_64 {
        let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
        let x86 = src.join("x86");
        build
            .define("RNN_ENABLE_X86_RTCD", None)
            .file(x86.join("x86cpu.c"))
            .file(x86.join("x86_dnn_map.c"));

        let variant = |name: &str, file: &str, defines: &[&str], flags: &[&str]| {
            let mut b = cc::Build::new();
            b.include(rn.join("include"))
                .include(&src)
                .define("RNNOISE_BUILD", None)
                .define("RNN_ENABLE_X86_RTCD", None)
                .warnings(false)
                .opt_level(2);
            for d in defines {
                b.define(d, None);
            }
            for f in flags {
                b.flag(f);
            }
            b.file(x86.join(file)).compile(name);
        };
        if msvc {
            // MSVC allows SSE intrinsics without flags but does not announce them; /arch:AVX2
            // defines __AVX__ and __AVX2__ itself.
            variant(
                "rnnoise_sse4_1",
                "nnet_sse4_1.c",
                &["__SSE__", "__SSE2__", "__SSE4_1__"],
                &[],
            );
            variant(
                "rnnoise_avx2",
                "nnet_avx2.c",
                &["__SSE__", "__SSE2__", "__SSE4_1__", "__FMA__"],
                &["/arch:AVX2"],
            );
        } else {
            variant("rnnoise_sse4_1", "nnet_sse4_1.c", &[], &["-msse4.1"]);
            variant("rnnoise_avx2", "nnet_avx2.c", &[], &["-mavx2", "-mfma"]);
        }
    }
    build.compile("rnnoise");

    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed={}", data_c.display());
    println!("cargo:rerun-if-env-changed=VIDAR_RNNOISE_DATA_DIR");
}
