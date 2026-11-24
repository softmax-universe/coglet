//! Engine-independent ABI v0 admission, shared by all real engines.
use crate::{Error, Limits, Result, nan};
use wasmparser::{
    ExternalKind, FuncType, Parser, Payload, TypeRef, ValType, Validator, WasmFeatures,
};

/// Frozen ABI 0.1 is the default. ABI 0.2 `coglet_v0.db` is never implied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// When true, admit exactly `coglet_v0.db: (i32,i32,i32,i32)->i32`.
    pub allow_db: bool,
}

impl Admission {
    pub const ABI_0_1: Self = Self { allow_db: false };
    pub const ABI_0_2_DB: Self = Self { allow_db: true };
}

/// 4-byte unsigned big-endian length prefix plus at most 65,536 reply bytes.
pub const DB_OUTPUT_CAPACITY: u32 = 65_540;
/// Maximum `coglet_v0.db` request bytes.
pub const DB_INPUT_MAX: u32 = 65_536;

fn invalid<T>(_: T) -> Error {
    Error::UnqualifiedModule
}
fn signature(params: &[ValType], results: &[ValType], ty: &FuncType) -> bool {
    ty.params() == params && ty.results() == results
}
/// ABI 0.1 admission: `coglet_v0.db` is always unqualified.
pub fn admit(bytes: &[u8], limits: &Limits) -> Result<()> {
    admit_with(bytes, limits, Admission::ABI_0_1)
}
/// Explicit ABI 0.2 DB opt-in. Callers that pass [`Admission::ABI_0_1`] match [`admit`].
pub fn admit_with(bytes: &[u8], limits: &Limits, admission: Admission) -> Result<()> {
    if bytes.len() > limits.runtime_bytes as usize / 4 {
        return Err(Error::MemoryExhausted);
    }
    let features = WasmFeatures::MUTABLE_GLOBAL
        | WasmFeatures::SIGN_EXTENSION
        | WasmFeatures::SATURATING_FLOAT_TO_INT
        | WasmFeatures::MULTI_VALUE
        | WasmFeatures::BULK_MEMORY
        | WasmFeatures::REFERENCE_TYPES
        | WasmFeatures::FLOATS;
    Validator::new_with_features(features)
        .validate_all(bytes)
        .map_err(invalid)?;
    let mut types = Vec::new();
    let mut functions = Vec::new();
    let mut memories = 0;
    let mut memory_export = false;
    let mut alloc = false;
    let mut handle = false;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(invalid)? {
            Payload::TypeSection(section) => {
                for ty in section.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(invalid)?);
                }
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(invalid)?;
                    let TypeRef::Func(index) = import.ty else {
                        return Err(Error::UnqualifiedModule);
                    };
                    let ty = types.get(index as usize).ok_or(Error::UnqualifiedModule)?;
                    if !allowed_import(import.module, import.name, ty, admission) {
                        return Err(Error::UnqualifiedModule);
                    }
                    functions.push(index);
                }
            }
            Payload::FunctionSection(section) => {
                for index in section {
                    functions.push(index.map_err(invalid)?);
                }
            }
            Payload::MemorySection(section) => {
                for memory in section {
                    let m = memory.map_err(invalid)?;
                    memories += 1;
                    if m.shared || m.memory64 {
                        return Err(Error::UnqualifiedModule);
                    }
                    if m.initial > u64::from(limits.memory_pages)
                        || m.maximum
                            .is_none_or(|max| max > u64::from(limits.memory_pages))
                    {
                        return Err(Error::MemoryExhausted);
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let e = export.map_err(invalid)?;
                    if e.name == "__post_instantiate" {
                        return Err(Error::UnqualifiedModule);
                    }
                    if e.name == "memory" {
                        memory_export = e.kind == ExternalKind::Memory && e.index == 0;
                    }
                    if ["abi_alloc", "handle", "_initialize", "__wasm_call_ctors"].contains(&e.name)
                    {
                        if e.kind != ExternalKind::Func {
                            return Err(Error::UnqualifiedModule);
                        }
                        let index = *functions
                            .get(e.index as usize)
                            .ok_or(Error::UnqualifiedModule)?;
                        let ty = types.get(index as usize).ok_or(Error::UnqualifiedModule)?;
                        use ValType::{I32, I64};
                        match e.name {
                            "abi_alloc" => {
                                alloc = signature(&[I32], &[I32], ty);
                                if !alloc {
                                    return Err(Error::UnqualifiedModule);
                                }
                            }
                            "handle" => {
                                handle = signature(&[I32, I32, I64, I64, I64], &[I32], ty);
                                if !handle {
                                    return Err(Error::UnqualifiedModule);
                                }
                            }
                            "_initialize" | "__wasm_call_ctors" => {
                                if !signature(&[], &[], ty) {
                                    return Err(Error::UnqualifiedModule);
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
            Payload::StartSection { .. } => return Err(Error::UnqualifiedModule),
            Payload::CodeSectionEntry(body) => {
                nan::validate(body.get_operators_reader().map_err(invalid)?)?;
            }
            _ => {}
        }
    }
    if memories != 1 || !memory_export || !alloc || !handle {
        return Err(Error::UnqualifiedModule);
    }
    Ok(())
}
fn allowed_import(module: &str, name: &str, ty: &FuncType, admission: Admission) -> bool {
    use ValType::{I32, I64};
    if module == "coglet_v0" {
        if name == "db" {
            return admission.allow_db && signature(&[I32, I32, I32, I32], &[I32], ty);
        }
        return ["timer", "emit", "llm", "log"].contains(&name)
            && signature(&[I32, I32], &[I32], ty);
    }
    if module != "wasi_snapshot_preview1" {
        return false;
    }
    let params: &[ValType] = match name {
        "args_get" | "args_sizes_get" | "environ_get" | "environ_sizes_get" | "fd_fdstat_get"
        | "fd_filestat_get" | "fd_prestat_get" => &[I32, I32],
        "fd_close" | "fd_datasync" | "fd_sync" => &[I32],
        "fd_advise" => &[I32, I64, I64, I32],
        "fd_allocate" => &[I32, I64, I64],
        "fd_fdstat_set_flags" => &[I32, I32],
        "fd_fdstat_set_rights" => &[I32, I64, I64],
        "fd_filestat_set_size" => &[I32, I64],
        "fd_filestat_set_times" => &[I32, I64, I64, I32],
        "fd_pread" | "fd_pwrite" => &[I32, I32, I32, I64, I32],
        "fd_prestat_dir_name" => &[I32, I32, I32],
        "fd_read" | "fd_write" => &[I32, I32, I32, I32],
        "fd_readdir" => &[I32, I32, I32, I64, I32],
        "fd_renumber" => &[I32, I32],
        "fd_seek" => &[I32, I64, I32, I32],
        "fd_tell" => &[I32, I32],
        "path_create_directory" | "path_remove_directory" | "path_unlink_file" => &[I32, I32, I32],
        "path_filestat_get" => &[I32, I32, I32, I32, I32],
        "path_filestat_set_times" => &[I32, I32, I32, I32, I64, I64, I32],
        "path_open" => &[I32, I32, I32, I32, I32, I64, I64, I32, I32],
        "path_readlink" => &[I32, I32, I32, I32, I32, I32],
        "path_rename" => &[I32, I32, I32, I32, I32, I32],
        "proc_exit" => return signature(&[I32], &[], ty),
        _ => return false,
    };
    signature(params, &[I32], ty)
}
