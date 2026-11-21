//! Neural noise suppressors behind `vidar_dsp::Denoiser`: RNNoise and DPDFNet (two sizes),
//! with fallback.

#[cfg(feature = "dpdfnet")]
pub mod dpdfnet;
#[cfg(feature = "dpdfnet")]
mod onnx;
mod rnnoise;

use std::path::PathBuf;

pub use rnnoise::{RnNoise, RNNOISE_DELAY_SAMPLES};
use vidar_dsp::Denoiser;
#[cfg(feature = "dpdfnet")]
pub use {dpdfnet::DpdfNet, dpdfnet::Vad, onnx::SpecModel};

/// A path in the source checkout (`relative` to the workspace root), for development builds
/// that run from `target\`. `None` for a release build running outside the checkout, so an
/// installed copy never looks in, or loads a DLL from, the folder it was built in.
pub fn repo_path(relative: &str) -> Option<PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let inside = || -> Option<bool> {
        let root = root.canonicalize().ok()?;
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        Some(exe.starts_with(root))
    };
    (cfg!(debug_assertions) || inside() == Some(true)).then(|| root.join(relative))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("model file {0} not found")]
    ModelMissing(String),
    #[error("model failed to load: {0}")]
    Init(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("{0} support is not compiled into this build")]
    Unsupported(&'static str),
}

/// Which noise model to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    RnNoise,
    /// DPDFNet with two DPRNN blocks (`dpdfnet2_48khz_hr`): Voice HD.
    DpdfNet,
    /// DPDFNet with eight DPRNN blocks (`dpdfnet8_48khz_hr`): Voice HD+. About 2.2 times the
    /// processor work of [`Engine::DpdfNet`], same delay.
    DpdfNet8,
}

impl Engine {
    /// Lenient parse; anything unknown means RNNoise.
    pub fn parse(s: &str) -> Engine {
        match s.trim().to_ascii_lowercase().as_str() {
            "dpdfnet" => Engine::DpdfNet,
            "dpdfnet8" => Engine::DpdfNet8,
            _ => Engine::RnNoise,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::RnNoise => "rnnoise",
            Engine::DpdfNet => "dpdfnet",
            Engine::DpdfNet8 => "dpdfnet8",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Engine::RnNoise => "RNNoise",
            Engine::DpdfNet => "DPDFNet",
            Engine::DpdfNet8 => "DPDFNet (large)",
        }
    }
    /// The next lighter engine to try when this one cannot load.
    pub fn lighter(self) -> Option<Engine> {
        match self {
            Engine::DpdfNet8 => Some(Engine::DpdfNet),
            Engine::DpdfNet => Some(Engine::RnNoise),
            Engine::RnNoise => None,
        }
    }
}

/// Everything needed to build a denoiser.
#[derive(Debug, Clone)]
pub struct DenoiseSpec {
    pub engine: Engine,
    /// DPDFNet speech detector: `"rnnoise"` or `"spectral"`.
    pub dpdfnet_vad: String,
    /// DPDFNet model name for [`Engine::DpdfNet`], e.g. `"dpdfnet2_48khz_hr"` (tools pick the
    /// size this way; [`Engine::DpdfNet8`] always loads `"dpdfnet8_48khz_hr"`).
    pub dpdfnet_model: String,
    /// Extra folders to search for `.onnx` models.
    pub model_dirs: Vec<PathBuf>,
    /// DPDFNet attenuation limit in dB (`None`: unlimited).
    pub dpdfnet_attn_limit_db: Option<f64>,
}

impl Default for DenoiseSpec {
    fn default() -> Self {
        Self {
            engine: Engine::RnNoise,
            dpdfnet_vad: "rnnoise".into(),
            dpdfnet_model: "dpdfnet2_48khz_hr".into(),
            model_dirs: Vec::new(),
            dpdfnet_attn_limit_db: None,
        }
    }
}

/// Build exactly the requested engine.
pub fn create(spec: &DenoiseSpec) -> Result<Box<dyn Denoiser>, Error> {
    match spec.engine {
        Engine::RnNoise => Ok(Box::new(RnNoise::new()?)),
        #[cfg(feature = "dpdfnet")]
        Engine::DpdfNet | Engine::DpdfNet8 => {
            let model = if spec.engine == Engine::DpdfNet8 {
                dpdfnet::LARGE_MODEL
            } else if dpdfnet::MODELS.contains(&spec.dpdfnet_model.as_str()) {
                spec.dpdfnet_model.as_str()
            } else {
                dpdfnet::DEFAULT_MODEL
            };
            let path = dpdfnet::find_model_path(model, &spec.model_dirs)?;
            let mut d = DpdfNet::load(&path, model, Vad::parse(&spec.dpdfnet_vad))?;
            d.set_attn_limit_db(spec.dpdfnet_attn_limit_db);
            Ok(Box::new(d))
        }
        #[cfg(not(feature = "dpdfnet"))]
        Engine::DpdfNet | Engine::DpdfNet8 => Err(Error::Unsupported("DPDFNet")),
    }
}

/// Build the requested engine, falling back to each lighter one in turn (Voice HD+, Voice
/// HD, RNNoise), then to none (DSP-only bypass). Returns the denoiser, the engine actually
/// running, and the errors met on the way.
pub fn create_with_fallback(
    spec: &DenoiseSpec,
) -> (Option<Box<dyn Denoiser>>, Option<Engine>, Vec<Error>) {
    let mut errors = Vec::new();
    let order = std::iter::successors(Some(spec.engine), |e| e.lighter());
    for engine in order {
        let attempt = DenoiseSpec {
            engine,
            ..spec.clone()
        };
        match create(&attempt) {
            Ok(d) => return (Some(d), Some(engine), errors),
            Err(e) => errors.push(e),
        }
    }
    (None, None, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_parse_is_lenient() {
        assert_eq!(Engine::parse(" DPDFNet "), Engine::DpdfNet);
        assert_eq!(Engine::parse("dpdfnet8"), Engine::DpdfNet8);
        for e in [Engine::RnNoise, Engine::DpdfNet, Engine::DpdfNet8] {
            assert_eq!(Engine::parse(e.as_str()), e);
        }
        assert_eq!(Engine::parse("bogus"), Engine::RnNoise);
        assert_eq!(Engine::parse(""), Engine::RnNoise);
    }

    #[test]
    fn missing_dpdfnet_model_falls_back_to_rnnoise() {
        let spec = DenoiseSpec {
            engine: Engine::DpdfNet,
            dpdfnet_model: "dpdfnet2_48khz_hr".into(),
            model_dirs: vec![PathBuf::from("Z:/definitely/missing")],
            ..DenoiseSpec::default()
        };
        // Make sure no stray env var points at real models.
        std::env::remove_var("VIDAR_MODELS_DIR");
        let (d, engine, errors) = create_with_fallback(&spec);
        assert!(d.is_some());
        assert_eq!(engine, Some(Engine::RnNoise));
        assert_eq!(errors.len(), 1);

        // Voice HD+ tries Voice HD on the way down.
        let (d, engine, errors) = create_with_fallback(&DenoiseSpec {
            engine: Engine::DpdfNet8,
            ..spec
        });
        assert!(d.is_some());
        assert_eq!(engine, Some(Engine::RnNoise));
        assert_eq!(errors.len(), 2);
    }
}
