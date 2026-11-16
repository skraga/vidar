//! Neural noise suppressors behind `vidar_dsp::Denoiser`: RNNoise and DPDFNet, with fallback.

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
    DpdfNet,
}

impl Engine {
    /// Lenient parse; anything unknown means RNNoise.
    pub fn parse(s: &str) -> Engine {
        match s.trim().to_ascii_lowercase().as_str() {
            "dpdfnet" => Engine::DpdfNet,
            _ => Engine::RnNoise,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::RnNoise => "rnnoise",
            Engine::DpdfNet => "dpdfnet",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Engine::RnNoise => "RNNoise",
            Engine::DpdfNet => "DPDFNet",
        }
    }
}

/// Everything needed to build a denoiser.
#[derive(Debug, Clone)]
pub struct DenoiseSpec {
    pub engine: Engine,
    /// DPDFNet speech detector: `"rnnoise"` or `"spectral"`.
    pub dpdfnet_vad: String,
    /// DPDFNet model name, e.g. `"dpdfnet2_48khz_hr"`.
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
        Engine::DpdfNet => {
            let model = if dpdfnet::MODELS.contains(&spec.dpdfnet_model.as_str()) {
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
        Engine::DpdfNet => Err(Error::Unsupported("DPDFNet")),
    }
}

/// Build the requested engine, falling back to RNNoise, then to none (DSP-only bypass).
/// Returns the denoiser, the engine actually running, and the errors met on the way.
pub fn create_with_fallback(
    spec: &DenoiseSpec,
) -> (Option<Box<dyn Denoiser>>, Option<Engine>, Vec<Error>) {
    let mut errors = Vec::new();
    let mut order = vec![spec.engine];
    if spec.engine != Engine::RnNoise {
        order.push(Engine::RnNoise);
    }
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
    }
}
