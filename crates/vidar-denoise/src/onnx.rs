//! ONNX inference for DPDFNet through ONNX Runtime (`ort`), loaded from onnxruntime.dll at
//! run time. (`tract` was tried and was 30-50 % off ONNX Runtime on identical inputs.)

use std::path::Path;
use std::sync::OnceLock;

use ort::environment::Environment;
use ort::memory::Allocator;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{IoBinding, Session};
use ort::value::Tensor;
use ort::AsPointer;

use crate::dpdfnet::FREQ_BINS;
use crate::Error;

/// One streaming step of a spectral model: (spectrum, state) -> (spectrum, state).
/// Spectra are `[FREQ_BINS][re, im]` flattened. The model owns its recurrent state and its
/// buffers, so a step neither allocates nor copies beyond what the model itself does.
pub trait SpecModel: Send {
    /// Buffer for the next input spectrum; fill it, then call [`SpecModel::step`].
    fn spec_in(&mut self) -> &mut [f32];
    /// Run one step and advance the state. On error the state is left unchanged.
    fn step(&mut self) -> Result<(), Error>;
    /// The spectrum produced by the last successful [`SpecModel::step`].
    fn spec_out(&self) -> &[f32];
    /// Return the state to its initial values.
    fn reset(&mut self);
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
    candidates.extend(crate::repo_path("third_party/onnxruntime/onnxruntime.dll"));
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

/// ONNX Runtime session with every input and output bound to buffers allocated at load.
///
/// The state ping-pongs between two tensors through two bindings: binding `i` reads
/// `states[i]` and writes `states[1 - i]`, so advancing the state is just switching
/// bindings. Runs go through the C API's `RunWithBinding` directly, because
/// `Session::run_binding` collects the outputs into newly allocated wrappers on every call.
struct OrtModel {
    session: Session,
    bindings: [IoBinding; 2],
    spec_in: Tensor<f32>,
    spec_out: Tensor<f32>,
    states: [Tensor<f32>; 2],
    init_state: Vec<f32>,
    /// Index of the state tensor that holds the current state.
    cur: usize,
}

impl OrtModel {
    fn new(session: Session, init_state: Vec<f32>) -> Result<Self, Error> {
        let alloc = Allocator::default();
        let in_spec = session.inputs()[0].name().to_string();
        let in_state = session.inputs()[1].name().to_string();
        let out_spec = session.outputs()[0].name().to_string();
        let out_state = session.outputs()[1].name().to_string();

        let mut spec_in = Tensor::<f32>::new(&alloc, [1usize, 1, FREQ_BINS, 2]).map_err(err)?;
        let mut spec_out = Tensor::<f32>::new(&alloc, [1usize, 1, FREQ_BINS, 2]).map_err(err)?;
        spec_in.extract_tensor_mut().1.fill(0.0);
        spec_out.extract_tensor_mut().1.fill(0.0);
        let mut states = [
            Tensor::<f32>::new(&alloc, [init_state.len()]).map_err(err)?,
            Tensor::<f32>::new(&alloc, [init_state.len()]).map_err(err)?,
        ];
        for s in &mut states {
            s.extract_tensor_mut().1.copy_from_slice(&init_state);
        }

        let mut bindings = [
            session.create_binding().map_err(err)?,
            session.create_binding().map_err(err)?,
        ];
        for (i, b) in bindings.iter_mut().enumerate() {
            // Owned CPU tensors are bound by reference: writes to `spec_in` are seen by
            // the next run, and outputs land directly in `spec_out` and the other state.
            b.bind_input(in_spec.as_str(), &spec_in).map_err(err)?;
            b.bind_input(in_state.as_str(), &states[i]).map_err(err)?;
            b.bind_output(out_spec.as_str(), &mut spec_out)
                .map_err(err)?;
            b.bind_output(out_state.as_str(), &mut states[1 - i])
                .map_err(err)?;
        }

        Ok(Self {
            session,
            bindings,
            spec_in,
            spec_out,
            states,
            init_state,
            cur: 0,
        })
    }
}

impl SpecModel for OrtModel {
    fn spec_in(&mut self) -> &mut [f32] {
        self.spec_in.extract_tensor_mut().1
    }

    fn step(&mut self) -> Result<(), Error> {
        let api = ort::api();
        // SAFETY: the session and binding pointers are valid for the lifetime of `self`,
        // and every bound tensor is owned by `self`. A non-null status is read and freed.
        unsafe {
            let status = (api.RunWithBinding)(
                self.session.ptr_mut(),
                std::ptr::null(),
                self.bindings[self.cur].ptr(),
            );
            if !status.0.is_null() {
                let msg = std::ffi::CStr::from_ptr((api.GetErrorMessage)(status.0))
                    .to_string_lossy()
                    .into_owned();
                (api.ReleaseStatus)(status.0);
                return Err(Error::Inference(msg));
            }
        }
        self.cur ^= 1;
        Ok(())
    }

    fn spec_out(&self) -> &[f32] {
        self.spec_out.extract_tensor().1
    }

    fn reset(&mut self) {
        self.cur = 0;
        self.states[0]
            .extract_tensor_mut()
            .1
            .copy_from_slice(&self.init_state);
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

/// Load a streaming DPDFNet model: single-threaded CPU session, full graph optimisation.
pub fn load(path: &Path) -> Result<Box<dyn SpecModel>, Error> {
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
        // Only affects ONNX Runtime's own worker threads; with one intra-op thread the model
        // runs on the caller, which `vidar_dsp::flush_denormals` covers.
        .with_flush_to_zero()
        .map_err(err)?
        .commit_from_file(path)
        .map_err(|e| Error::Init(e.to_string()))?;

    if session.inputs().len() < 2 || session.outputs().len() < 2 {
        return Err(Error::Init(
            "expected a streaming model with (spec, state) inputs and outputs".into(),
        ));
    }
    let init = {
        let meta = session.metadata().map_err(err)?;
        initial_state(|k| meta.custom(k))?
    };
    Ok(Box::new(OrtModel::new(session, init)?))
}
