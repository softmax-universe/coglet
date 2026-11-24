//! Pure original→output rewrite. No keys, network, registry or clock.
use crate::{
    F32_CANON, F64_CANON, PROFILE, Refuse, profile_bytes, sha256_hex, transform_artifact_bytes,
};
use wasm_encoder::reencode::{Error as ReencodeError, Reencode};
use wasm_encoder::{CodeSection, Function, Ieee32, Ieee64, Instruction as I, Module, ValType};
use wasmparser::{Encoding, FuncType, Parser, Payload, TypeRef, Validator, WasmFeatures};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins {
    pub profile: String,
    pub max_bytes: usize,
    pub max_ops: u32,
}

impl Default for Pins {
    fn default() -> Self {
        Self {
            profile: PROFILE.into(),
            max_bytes: 1_048_576,
            max_ops: 100_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestRecord {
    pub profile: String,
    pub original_sha256: String,
    pub output_sha256: String,
    pub transform_artifact_sha256: String,
    pub profile_bytes_sha256: String,
    pub wrapped: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    pub original: Vec<u8>,
    pub output: Vec<u8>,
    pub record: DigestRecord,
}

pub fn rewrite(original: &[u8], pins: &Pins) -> Result<Rewrite, Refuse> {
    if pins.profile != PROFILE {
        return Err(Refuse::Unsupported("profile"));
    }
    if original.len() > pins.max_bytes {
        return Err(Refuse::Limit);
    }
    admit_subset(original, pins)?;
    let wrapped = count_wraps(original)?;
    if wrapped > pins.max_ops {
        return Err(Refuse::Limit);
    }
    let output = if wrapped == 0 {
        original.to_vec()
    } else {
        canonicalize(original)?
    };
    if output.len() > pins.max_bytes {
        return Err(Refuse::Limit);
    }
    admit_subset(&output, pins)?;
    let record = DigestRecord {
        profile: PROFILE.into(),
        original_sha256: sha256_hex(original),
        output_sha256: sha256_hex(&output),
        transform_artifact_sha256: sha256_hex(transform_artifact_bytes()),
        profile_bytes_sha256: sha256_hex(profile_bytes()),
        wrapped,
    };
    Ok(Rewrite {
        original: original.to_vec(),
        output,
        record,
    })
}

fn features() -> WasmFeatures {
    WasmFeatures::MUTABLE_GLOBAL
        | WasmFeatures::SIGN_EXTENSION
        | WasmFeatures::SATURATING_FLOAT_TO_INT
        | WasmFeatures::MULTI_VALUE
        | WasmFeatures::BULK_MEMORY
        | WasmFeatures::REFERENCE_TYPES
        | WasmFeatures::FLOATS
}

fn admit_subset(bytes: &[u8], pins: &Pins) -> Result<(), Refuse> {
    Validator::new_with_features(features())
        .validate_all(bytes)
        .map_err(|_| Refuse::Malformed)?;
    let mut types = Vec::new();
    let mut ops = 0u32;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|_| Refuse::Malformed)? {
            Payload::Version { encoding, .. } => {
                if encoding != Encoding::Module {
                    return Err(Refuse::Unsupported("component"));
                }
            }
            Payload::TypeSection(section) => {
                for ty in section.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(|_| Refuse::Unsupported("gc"))?);
                }
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(|_| Refuse::Malformed)?;
                    let TypeRef::Func(index) = import.ty else {
                        return Err(Refuse::Unsupported("non-func-import"));
                    };
                    let ty = types.get(index as usize).ok_or(Refuse::Malformed)?;
                    if !allowed_import(import.module, import.name, ty) {
                        return Err(Refuse::Unsupported("import"));
                    }
                }
            }
            Payload::MemorySection(section) => {
                for memory in section {
                    let m = memory.map_err(|_| Refuse::Malformed)?;
                    if m.shared {
                        return Err(Refuse::Unsupported("shared-memory"));
                    }
                    if m.memory64 {
                        return Err(Refuse::Unsupported("memory64"));
                    }
                }
            }
            Payload::StartSection { .. } => return Err(Refuse::Unsupported("start")),
            Payload::CodeSectionEntry(body) => {
                let reader = body.get_operators_reader().map_err(|_| Refuse::Malformed)?;
                for op in reader {
                    let op = op.map_err(|_| Refuse::Malformed)?;
                    ops = ops.checked_add(1).ok_or(Refuse::Limit)?;
                    if ops > pins.max_ops {
                        return Err(Refuse::Limit);
                    }
                    let _ = op;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn allowed_import(module: &str, name: &str, ty: &FuncType) -> bool {
    use wasmparser::ValType::I32;
    let sig = |p: &[wasmparser::ValType], r: &[wasmparser::ValType]| {
        ty.params() == p && ty.results() == r
    };
    if module == "coglet_v0" {
        return ["timer", "emit", "llm", "log"].contains(&name) && sig(&[I32, I32], &[I32]);
    }
    if module != "wasi_snapshot_preview1" {
        return false;
    }
    matches!(
        name,
        "args_get"
            | "args_sizes_get"
            | "environ_get"
            | "environ_sizes_get"
            | "fd_close"
            | "fd_datasync"
            | "fd_sync"
            | "fd_fdstat_get"
            | "fd_filestat_get"
            | "fd_prestat_get"
            | "fd_prestat_dir_name"
            | "fd_read"
            | "fd_write"
            | "fd_seek"
            | "path_open"
            | "path_create_directory"
            | "path_remove_directory"
            | "path_unlink_file"
            | "proc_exit"
    )
}

fn wrap_kind(op: &wasmparser::Operator<'_>) -> Option<u8> {
    use wasmparser::Operator::*;
    match op {
        F32Add | F32Sub | F32Mul | F32Div | F32Sqrt | F32Ceil | F32Floor | F32Trunc
        | F32Nearest | F32Min | F32Max | F32DemoteF64 => Some(32),
        F64Add | F64Sub | F64Mul | F64Div | F64Sqrt | F64Ceil | F64Floor | F64Trunc
        | F64Nearest | F64Min | F64Max | F64PromoteF32 => Some(64),
        _ => None,
    }
}

fn count_wraps(bytes: &[u8]) -> Result<u32, Refuse> {
    let mut n = 0u32;
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::CodeSectionEntry(body) = payload.map_err(|_| Refuse::Malformed)? {
            let reader = body.get_operators_reader().map_err(|_| Refuse::Malformed)?;
            for op in reader {
                if wrap_kind(&op.map_err(|_| Refuse::Malformed)?).is_some() {
                    n += 1;
                }
            }
        }
    }
    Ok(n)
}

fn param_counts(bytes: &[u8]) -> Result<Vec<u32>, Refuse> {
    let mut types = Vec::new();
    let mut fn_types = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|_| Refuse::Malformed)? {
            Payload::TypeSection(section) => {
                for ty in section.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(|_| Refuse::Malformed)?.params().len() as u32);
                }
            }
            Payload::FunctionSection(section) => {
                for t in section {
                    fn_types.push(t.map_err(|_| Refuse::Malformed)?);
                }
            }
            _ => {}
        }
    }
    fn_types
        .into_iter()
        .map(|t| types.get(t as usize).copied().ok_or(Refuse::Malformed))
        .collect()
}

struct Canon {
    nparams: Vec<u32>,
    next: usize,
}

impl Reencode for Canon {
    type Error = std::convert::Infallible;

    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        func: wasmparser::FunctionBody<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        let mut n = self.nparams[self.next];
        self.next += 1;
        let mut locals = Vec::new();
        for local in func.get_locals_reader()? {
            let (c, t) = local?;
            n += c;
            locals.push((c, self.val_type(t)?));
        }
        let needs = {
            let mut yes = false;
            for op in func.get_operators_reader()? {
                if wrap_kind(&op?).is_some() {
                    yes = true;
                    break;
                }
            }
            yes
        };
        if needs {
            locals.push((1, ValType::F32));
            locals.push((1, ValType::F64));
        }
        let (t32, t64) = (n, n + 1);
        let mut f = Function::new(locals);
        for op in func.get_operators_reader()? {
            let op = op?;
            let kind = wrap_kind(&op);
            f.instruction(&self.instruction(op)?);
            match kind {
                Some(32) => {
                    f.instruction(&I::LocalSet(t32));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::F32Const(Ieee32::from(f32::from_bits(F32_CANON))));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::LocalGet(t32));
                    f.instruction(&I::F32Eq);
                    f.instruction(&I::Select);
                }
                Some(64) => {
                    f.instruction(&I::LocalSet(t64));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::F64Const(Ieee64::from(f64::from_bits(F64_CANON))));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::LocalGet(t64));
                    f.instruction(&I::F64Eq);
                    f.instruction(&I::Select);
                }
                _ => {}
            }
        }
        code.function(&f);
        Ok(())
    }
}

fn canonicalize(bytes: &[u8]) -> Result<Vec<u8>, Refuse> {
    let nparams = param_counts(bytes)?;
    let mut c = Canon { nparams, next: 0 };
    let mut module = Module::new();
    c.parse_core_module(&mut module, Parser::new(0), bytes)
        .map_err(|_| Refuse::Malformed)?;
    Ok(module.finish())
}
