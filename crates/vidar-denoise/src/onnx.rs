//! ONNX inference for DPDFNet through ONNX Runtime (`ort`), loaded from onnxruntime.dll at
//! run time. (`tract` was tried and was 30-50 % off ONNX Runtime on identical inputs.)

use std::path::Path;
use std::sync::OnceLock;

use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

use crate::dpdfnet::FREQ_BINS;
use crate::Error;

/// One streaming step of a spectral model: (spectrum, state) -> (spectrum, state).
/// Spectra are `[FREQ_BINS][re, im]` flattened.
pub trait SpecModel: Send {
    fn run(
        &mut self,
        spec: &[f32],
        state: &[f32],
        spec_out: &mut [f32],
        state_out: &mut [f32],
    ) -> Result<(), Error>;
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::Inference(e.to_string())
}

/// Where `onnxruntime.dll` is looked for: `ORT_DYLIB_PATH`, next to the executable, then the
/// development copy fetched by `cargo xtask fetch`. The DLL is loaded only when DPDFNet is
/// used, so RNNoise-only installs do not need it.
pub fn onnxruntime_path() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH") {
        candidates.push(std::path::PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("onnxruntime.dll"));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/onnxruntime/onnxruntime.dll"),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// One ONNX Runtime environment for the whole process.
fn environment() -> Result<&'static Environment, Error> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let dll = onnxruntime_path().ok_or_else(|| "onnxruntime.dll not found".to_string())?;
        ort::init_from(&dll)
            .map_err(|e| format!("cannot load {}: {e}", dll.display()))?
            .with_name("vidar")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| Error::Init(e.clone()))
}

struct OrtModel {
    session: Session,
    in_spec: String,
    in_state: String,
    state_len: usize,
}

impl SpecModel for OrtModel {
    fn run(
        &mut self,
        spec: &[f32],
        state: &[f32],
        spec_out: &mut [f32],
        state_out: &mut [f32],
    ) -> Result<(), Error> {
        let spec_t = TensorRef::from_array_view(([1usize, 1, FREQ_BINS, 2], spec)).map_err(err)?;
        let state_t = TensorRef::from_array_view(([self.state_len], state)).map_err(err)?;
        let outputs = self
            .session
            .run(ort::inputs![self.in_spec.as_str() => spec_t, self.in_state.as_str() => state_t])
            .map_err(err)?;
        let (_, s) = outputs[0].try_extract_tensor::<f32>().map_err(err)?;
        let (_, st) = outputs[1].try_extract_tensor::<f32>().map_err(err)?;
        if s.len() != spec_out.len() || st.len() != state_out.len() {
            return Err(Error::Inference("unexpected output size".into()));
        }
        spec_out.copy_from_slice(s);
        state_out.copy_from_slice(st);
        Ok(())
    }
}

/// Initial state from the model's metadata: ERB norm values, then spectral norm values, then zeros.
fn initial_state(meta: impl Fn(&str) -> Option<String>) -> Result<Vec<f32>, Error> {
    let get =
        |key: &str| meta(key).ok_or_else(|| Error::Init(format!("model metadata '{key}' missing")));
    let parse_usize = |key: &str| -> Result<usize, Error> {
        get(key)?
            .trim()
            .parse()
            .map_err(|_| Error::Init(format!("bad metadata '{key}'")))
    };
    let parse_list = |key: &str| -> Result<Vec<f32>, Error> {
        get(key)?
            .split(',')
            .map(|v| {
                v.trim()
                    .parse::<f32>()
                    .map_err(|_| Error::Init(format!("bad value in '{key}'")))
            })
            .collect()
    };

    let state_size = parse_usize("state_size")?;
    let erb_n = parse_usize("erb_norm_state_size")?;
    let spec_n = parse_usize("spec_norm_state_size")?;
    let erb = parse_list("erb_norm_init")?;
    let spec = parse_list("spec_norm_init")?;
    if erb.len() != erb_n || spec.len() != spec_n || erb_n + spec_n > state_size {
        return Err(Error::Init("model metadata sizes do not match".into()));
    }
    let mut state = vec![0.0f32; state_size];
    state[..erb_n].copy_from_slice(&erb);
    state[erb_n..erb_n + spec_n].copy_from_slice(&spec);
    Ok(state)
}

/// Load a streaming DPDFNet model: single-threaded CPU session, full graph optimisation
/// Returns the model and its initial state.
pub fn load(path: &Path) -> Result<(Box<dyn SpecModel>, Vec<f32>), Error> {
    let env = environment()?;
    let session = Session::builder(env)
        .map_err(err)?
        .with_intra_threads(1)
        .map_err(err)?
        .with_inter_threads(1)
        .map_err(err)?
        .with_parallel_execution(false)
        .map_err(err)?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(err)?
        .commit_from_file(path)
        .map_err(|e| Error::Init(e.to_string()))?;

    if session.inputs().len() < 2 || session.outputs().len() < 2 {
        return Err(Error::Init(
            "expected a streaming model with (spec, state) inputs and outputs".into(),
        ));
    }
    let in_spec = session.inputs()[0].name().to_string();
    let in_state = session.inputs()[1].name().to_string();

    let init = {
        let meta = session.metadata().map_err(err)?;
        initial_state(|k| meta.custom(k))?
    };
    let state_len = init.len();
    Ok((
        Box::new(OrtModel {
            session,
            in_spec,
            in_state,
            state_len,
        }),
        init,
    ))
}
