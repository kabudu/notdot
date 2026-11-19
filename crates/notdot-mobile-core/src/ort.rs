//! Safe Rust over `native/ort_shim.c`: CPU inference with the ONNX Runtime
//! that sherpa-onnx already ships. The shim is compiled against the pinned
//! official header and does all ONNX Runtime calls through its typed API
//! struct; this side only validates and marshals.
//!
//! Inputs borrow caller memory for one [`Session::run`]; element counts are
//! overflow-checked here and again in the shim. The single output is copied
//! into a `Vec` and the runtime's buffers are released before returning.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    ptr,
};

const F32: c_int = 1;
const I64: c_int = 2;
const BOOL: c_int = 3;
const MAX_RANK: usize = 8;
const ERROR_BYTES: usize = 512;

#[repr(C)]
struct NdSession {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn nd_ort_open(
        path: *const c_char,
        threads: c_int,
        err: *mut c_char,
        err_len: usize,
    ) -> *mut NdSession;
    fn nd_ort_close(session: *mut NdSession);
    #[allow(clippy::too_many_arguments)]
    fn nd_ort_run(
        session: *mut NdSession,
        n_in: usize,
        names: *const *const c_char,
        types: *const c_int,
        data: *const *const c_void,
        bytes: *const usize,
        shapes: *const *const i64,
        ranks: *const usize,
        output_name: *const c_char,
        max_out: usize,
        out_data: *mut *mut f32,
        out_len: *mut usize,
        out_shape: *mut i64,
        out_rank: *mut usize,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    fn nd_ort_free(data: *mut f32);
    #[cfg(test)]
    fn nd_ort_version() -> *const c_char;
    fn nd_ort_count(session: *mut NdSession, outputs: c_int) -> usize;
    #[allow(clippy::too_many_arguments)]
    fn nd_ort_describe(
        session: *mut NdSession,
        outputs: c_int,
        index: usize,
        name: *mut c_char,
        name_len: usize,
        kind: *mut c_int,
        dims: *mut i64,
        rank: *mut usize,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    #[allow(clippy::too_many_arguments)]
    fn nd_ort_run_many(
        session: *mut NdSession,
        n_in: usize,
        names: *const *const c_char,
        types: *const c_int,
        data: *const *const c_void,
        bytes: *const usize,
        shapes: *const *const i64,
        ranks: *const usize,
        n_out: usize,
        out_names: *const *const c_char,
        max_out: usize,
        max_total: usize,
        out_types: *mut c_int,
        out_data: *mut *mut c_void,
        out_len: *mut usize,
        out_shapes: *mut i64,
        out_ranks: *mut usize,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    fn nd_ort_free_any(data: *mut c_void);
}

/// An input tensor borrowing caller memory for one `run`.
pub enum Input<'a> {
    F32(&'a [f32], &'a [i64]),
    I64(&'a [i64], &'a [i64]),
    #[cfg_attr(not(test), allow(dead_code))]
    Bool(&'a [bool], &'a [i64]),
}

impl Input<'_> {
    /// (type, data pointer, bytes, shape), after checking the shape
    /// describes exactly the data given.
    fn raw(&self) -> Result<(c_int, *const c_void, usize, &[i64]), String> {
        let (kind, data, len, size, shape) = match self {
            Input::F32(d, s) => (F32, d.as_ptr() as *const c_void, d.len(), 4, *s),
            Input::I64(d, s) => (I64, d.as_ptr() as *const c_void, d.len(), 8, *s),
            // Rust's bool is one byte holding 0 or 1, as ONNX Runtime expects.
            Input::Bool(d, s) => (BOOL, d.as_ptr() as *const c_void, d.len(), 1, *s),
        };
        if shape.len() > MAX_RANK {
            return Err("Wake model input has too many dimensions".into());
        }
        let elements = shape.iter().try_fold(1usize, |n, &d| {
            usize::try_from(d).ok().and_then(|d| n.checked_mul(d))
        });
        if elements != Some(len) {
            return Err("Wake model input size does not match its shape".into());
        }
        let bytes = len
            .checked_mul(size)
            .ok_or("Wake model input is too large")?;
        Ok((kind, data, bytes, shape))
    }
}

/// A float output, copied out of the runtime.
pub struct Output {
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

fn message(buffer: &[u8]) -> String {
    CStr::from_bytes_until_nul(buffer)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown ONNX Runtime error".into())
}

/// One CPU inference session.
pub struct Session(*mut NdSession);
// SAFETY: an OrtSession is used by one thread at a time (`run` takes
// `&mut self`); the scorer owns it on its worker thread.
unsafe impl Send for Session {}

impl Session {
    pub fn open(path: &Path, threads: i32) -> Result<Self, String> {
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid wake model path".to_owned())?;
        let mut err = [0u8; ERROR_BYTES];
        // SAFETY: `path` and `err` are valid for the call; the shim writes
        // at most `err.len()` bytes, NUL-terminated.
        let session =
            unsafe { nd_ort_open(path.as_ptr(), threads, err.as_mut_ptr().cast(), err.len()) };
        if session.is_null() {
            return Err(format!("Could not load the wake model: {}", message(&err)));
        }
        Ok(Self(session))
    }

    /// Run with `inputs`; copy out the float output `output` if it has at
    /// most `max_out` elements.
    pub fn run(
        &mut self,
        inputs: &[(&str, Input<'_>)],
        output: &str,
        max_out: usize,
    ) -> Result<Output, String> {
        if inputs.is_empty() || inputs.len() > 64 {
            return Err("Wrong number of wake model inputs".into());
        }
        let names = inputs
            .iter()
            .map(|(n, _)| CString::new(*n).map_err(|_| "Invalid wake model input name".to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        let output =
            CString::new(output).map_err(|_| "Invalid wake model output name".to_owned())?;
        let raw = inputs
            .iter()
            .map(|(_, input)| input.raw())
            .collect::<Result<Vec<_>, _>>()?;
        let name_ptrs: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
        let types: Vec<c_int> = raw.iter().map(|r| r.0).collect();
        let data: Vec<*const c_void> = raw.iter().map(|r| r.1).collect();
        let bytes: Vec<usize> = raw.iter().map(|r| r.2).collect();
        let shapes: Vec<*const i64> = raw.iter().map(|r| r.3.as_ptr()).collect();
        let ranks: Vec<usize> = raw.iter().map(|r| r.3.len()).collect();
        let mut out_data: *mut f32 = ptr::null_mut();
        let (mut out_len, mut out_rank) = (0usize, 0usize);
        let mut out_shape = [0i64; MAX_RANK];
        let mut err = [0u8; ERROR_BYTES];
        // SAFETY: every array has `inputs.len()` entries; each data pointer
        // and shape outlives the call and was checked to match its byte
        // length; `out_shape` holds MAX_RANK entries, the shim's limit.
        let status = unsafe {
            nd_ort_run(
                self.0,
                inputs.len(),
                name_ptrs.as_ptr(),
                types.as_ptr(),
                data.as_ptr(),
                bytes.as_ptr(),
                shapes.as_ptr(),
                ranks.as_ptr(),
                output.as_ptr(),
                max_out,
                &mut out_data,
                &mut out_len,
                out_shape.as_mut_ptr(),
                &mut out_rank,
                err.as_mut_ptr().cast(),
                err.len(),
            )
        };
        if status != 0 {
            return Err(format!("Wake model error: {}", message(&err)));
        }
        let data = if out_len == 0 {
            Vec::new()
        } else if out_data.is_null() || out_len > max_out || out_rank > MAX_RANK {
            // Not produced by the shim; release anything it returned.
            // SAFETY: null, or the shim's malloc'd buffer, freed once.
            unsafe { nd_ort_free(out_data) };
            return Err("Unexpected wake model output".into());
        } else {
            // SAFETY: on success the shim returns a malloc'd buffer of
            // exactly `out_len` floats (bounded by `max_out`), freed below.
            let copy = unsafe { std::slice::from_raw_parts(out_data, out_len) }.to_vec();
            unsafe { nd_ort_free(out_data) };
            copy
        };
        Ok(Output {
            shape: out_shape[..out_rank.min(MAX_RANK)].to_vec(),
            data,
        })
    }

    /// The model's inputs (or outputs): names, element types and shapes
    /// (-1 for a dynamic dimension).
    pub fn describe(&mut self, outputs: bool) -> Result<Vec<TensorInfo>, String> {
        // SAFETY: a live session; the call only reads it.
        let count = unsafe { nd_ort_count(self.0, c_int::from(outputs)) };
        if count == 0 || count > MAX_TENSORS {
            return Err("Wake model has an unexpected number of tensors".into());
        }
        (0..count)
            .map(|i| {
                let mut name = [0u8; NAME_BYTES];
                let (mut kind, mut rank) = (0 as c_int, 0usize);
                let mut dims = [0i64; MAX_RANK];
                let mut err = [0u8; ERROR_BYTES];
                // SAFETY: every buffer is valid for its stated length; the
                // shim writes a NUL-terminated name shorter than NAME_BYTES,
                // at most MAX_RANK dims, and an error string within err.
                let status = unsafe {
                    nd_ort_describe(
                        self.0,
                        c_int::from(outputs),
                        i,
                        name.as_mut_ptr().cast(),
                        name.len(),
                        &mut kind,
                        dims.as_mut_ptr(),
                        &mut rank,
                        err.as_mut_ptr().cast(),
                        err.len(),
                    )
                };
                if status != 0 || rank > MAX_RANK {
                    return Err(format!("Wake model error: {}", message(&err)));
                }
                Ok(TensorInfo {
                    name: message(&name),
                    kind: match kind {
                        F32 => Some(Kind::F32),
                        I64 => Some(Kind::I64),
                        BOOL => Some(Kind::Bool),
                        _ => None,
                    },
                    dims: dims[..rank].to_vec(),
                })
            })
            .collect()
    }

    /// Run with `inputs` and copy out the named float or int64 `outputs`,
    /// each at most `max_out` elements and together at most `max_total`.
    /// Every runtime buffer is freed before returning, on success or failure.
    pub fn run_many(
        &mut self,
        inputs: &[(&str, Input<'_>)],
        outputs: &[&str],
        max_out: usize,
        max_total: usize,
    ) -> Result<Vec<Tensor>, String> {
        if inputs.is_empty()
            || inputs.len() > MAX_TENSORS
            || outputs.is_empty()
            || outputs.len() > MAX_TENSORS
        {
            return Err("Wrong number of wake model tensors".into());
        }
        let names = inputs
            .iter()
            .map(|(n, _)| CString::new(*n).map_err(|_| "Invalid wake model input name".to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        let out_names = outputs
            .iter()
            .map(|n| CString::new(*n).map_err(|_| "Invalid wake model output name".to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        let raw = inputs
            .iter()
            .map(|(_, input)| input.raw())
            .collect::<Result<Vec<_>, _>>()?;
        let name_ptrs: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
        let out_ptrs: Vec<*const c_char> = out_names.iter().map(|n| n.as_ptr()).collect();
        let types: Vec<c_int> = raw.iter().map(|r| r.0).collect();
        let data: Vec<*const c_void> = raw.iter().map(|r| r.1).collect();
        let bytes: Vec<usize> = raw.iter().map(|r| r.2).collect();
        let shapes: Vec<*const i64> = raw.iter().map(|r| r.3.as_ptr()).collect();
        let ranks: Vec<usize> = raw.iter().map(|r| r.3.len()).collect();
        let n = outputs.len();
        let mut out_types = vec![0 as c_int; n];
        let mut out_data: Vec<*mut c_void> = vec![ptr::null_mut(); n];
        let mut out_len = vec![0usize; n];
        let mut out_shapes = vec![0i64; n * MAX_RANK];
        let mut out_ranks = vec![0usize; n];
        let mut err = [0u8; ERROR_BYTES];
        // SAFETY: input arrays have inputs.len() entries and output arrays n
        // (out_shapes n * MAX_RANK); every input's data and shape outlives
        // the call and was checked to match its byte length.
        let status = unsafe {
            nd_ort_run_many(
                self.0,
                inputs.len(),
                name_ptrs.as_ptr(),
                types.as_ptr(),
                data.as_ptr(),
                bytes.as_ptr(),
                shapes.as_ptr(),
                ranks.as_ptr(),
                n,
                out_ptrs.as_ptr(),
                max_out,
                max_total,
                out_types.as_mut_ptr(),
                out_data.as_mut_ptr(),
                out_len.as_mut_ptr(),
                out_shapes.as_mut_ptr(),
                out_ranks.as_mut_ptr(),
                err.as_mut_ptr().cast(),
                err.len(),
            )
        };
        // Take ownership of every buffer first, so all are freed whatever happens.
        let owned: Vec<Owned> = out_data.iter().map(|p| Owned(*p)).collect();
        if status != 0 {
            return Err(format!("Wake model error: {}", message(&err)));
        }
        if out_len
            .iter()
            .try_fold(0usize, |acc, &l| acc.checked_add(l))
            .is_none_or(|t| t > max_total)
        {
            return Err("Unexpected wake model output".into());
        }
        (0..n)
            .map(|k| {
                let len = out_len[k];
                let rank = out_ranks[k];
                if len > max_out || rank > MAX_RANK || (len > 0 && owned[k].0.is_null()) {
                    return Err("Unexpected wake model output".to_owned());
                }
                let shape = out_shapes[k * MAX_RANK..k * MAX_RANK + rank].to_vec();
                let count = shape.iter().try_fold(1usize, |acc, &d| {
                    usize::try_from(d).ok().and_then(|d| acc.checked_mul(d))
                });
                if count != Some(len) {
                    return Err("Unexpected wake model output".to_owned());
                }
                // SAFETY: the shim returned a malloc'd buffer of exactly len
                // elements of the reported type (null only when len is 0).
                let data = match out_types[k] {
                    F32 => TensorData::F32(if len == 0 {
                        Vec::new()
                    } else {
                        unsafe { std::slice::from_raw_parts(owned[k].0.cast::<f32>(), len) }
                            .to_vec()
                    }),
                    I64 => TensorData::I64(if len == 0 {
                        Vec::new()
                    } else {
                        unsafe { std::slice::from_raw_parts(owned[k].0.cast::<i64>(), len) }
                            .to_vec()
                    }),
                    _ => return Err("Unexpected wake model output type".to_owned()),
                };
                Ok(Tensor { shape, data })
            })
            .collect()
    }
}

const MAX_TENSORS: usize = 64;
const NAME_BYTES: usize = 128;

/// A buffer returned by the shim, freed on drop.
struct Owned(*mut c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: null, or a malloc'd buffer from nd_ort_run_many, freed once.
        unsafe { nd_ort_free_any(self.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    F32,
    I64,
    Bool,
}

/// One model input or output as the model declares it.
#[derive(Clone, Debug, PartialEq)]
pub struct TensorInfo {
    pub name: String,
    /// None for an element type the shim does not support.
    pub kind: Option<Kind>,
    pub dims: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TensorData {
    F32(Vec<f32>),
    I64(Vec<i64>),
}

/// An output copied out of the runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub shape: Vec<i64>,
    pub data: TensorData,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: created by nd_ort_open and closed once.
        unsafe { nd_ort_close(self.0) }
    }
}

/// The linked runtime's version, for diagnostics.
#[cfg(test)]
pub fn version() -> String {
    // SAFETY: a static, NUL-terminated string owned by the runtime.
    unsafe { CStr::from_ptr(nd_ort_version()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_must_describe_the_data_exactly() {
        let data = [1f32, 2., 3., 4.];
        assert!(Input::F32(&data, &[2, 2]).raw().is_ok());
        assert!(Input::F32(&data, &[4]).raw().is_ok());
        assert!(Input::F32(&data, &[3]).raw().is_err());
        assert!(Input::F32(&data, &[2, -2]).raw().is_err());
        assert!(Input::F32(&data, &[i64::MAX, i64::MAX]).raw().is_err());
        assert!(Input::F32(&[], &[1, 8, 0, 52]).raw().is_ok());
        assert!(
            Input::F32(&data, &[1, 1, 1, 1, 1, 1, 1, 1, 4])
                .raw()
                .is_err()
        );
        assert!(Input::Bool(&[false], &[1]).raw().is_ok());
        assert!(Input::I64(&[1, 2], &[1, 2]).raw().is_ok());
    }

    #[test]
    fn the_linked_runtime_is_reachable_and_bad_models_are_refused() {
        assert!(version().starts_with("1."), "{}", version());
        let missing = Session::open(Path::new("/nonexistent/model.onnx"), 1);
        assert!(missing.is_err());
        assert!(Session::open(Path::new("/nonexistent/model.onnx"), 0).is_err());
    }
}
