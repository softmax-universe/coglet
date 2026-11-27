//! Per-step durable diagnostics for the required R3 native replay.
//!
//! Captures only outcomes that `run_next` actually RETURNED (Ok(Some), Ok(None),
//! Err). No guarantee exists for engine-init failures or process crashes. Files
//! live in a sibling `<runtime-dir>-diagnostics` directory, are created with
//! `create_new` (never overwritten), fsynced, and the directory is fsynced.
//! Any persistence failure is fatal (fail closed). Diagnostics are evidence for
//! humans only; they never count toward qualification.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt::Debug,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `<dir>-diagnostics`, a sibling of the runtime evidence directory.
pub fn sibling_dir(runtime_dir: &Path) -> PathBuf {
    let mut name = runtime_dir
        .file_name()
        .expect("runtime dir has a final component")
        .to_os_string();
    name.push("-diagnostics");
    runtime_dir.with_file_name(name)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Immutable pins computed BEFORE any execution.
#[derive(Clone, Debug)]
pub struct Pins {
    pub wasm_sha256: String,
    pub setup_sha256: String,
    pub publication_sha256: String,
}

impl Pins {
    pub fn from_publication_dir(dir: &Path) -> io::Result<Self> {
        Ok(Self {
            wasm_sha256: sha256_hex(&fs::read(dir.join("step.wasm"))?),
            setup_sha256: sha256_hex(&fs::read(dir.join("runtime-setup.json"))?),
            publication_sha256: sha256_hex(&fs::read(dir.join("publication.json"))?),
        })
    }
}

pub struct Diagnostics {
    dir: PathBuf,
    arch: String,
    pins: Pins,
}

impl Diagnostics {
    pub fn create(runtime_dir: &Path, arch: &str, pins: Pins) -> io::Result<Self> {
        Self::create_in(sibling_dir(runtime_dir), arch, pins)
    }

    pub fn create_in(dir: PathBuf, arch: &str, pins: Pins) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        sync_dir(&dir)?;
        if let Some(parent) = dir.parent() {
            sync_dir(parent)?;
        }
        Ok(Self {
            dir,
            arch: arch.to_owned(),
            pins,
        })
    }

    #[allow(dead_code)] // Used only by the separate synthetic control target.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn step_path(&self, step: usize) -> PathBuf {
        self.dir.join(format!("step-{step}.json"))
    }

    /// Retain the COMPLETE original outcome. Returns the written path.
    pub fn persist<E: Debug>(
        &self,
        step: usize,
        context: &Value,
        outcome: &Result<Option<Value>, E>,
    ) -> io::Result<PathBuf> {
        let (status, result, error) = match outcome {
            Ok(Some(v)) => ("ok_some", v.clone(), Value::Null),
            Ok(None) => ("ok_none", Value::Null, Value::Null),
            Err(e) => ("err", Value::Null, json!(format!("{e:?}"))),
        };
        let doc = json!({
            "schema": "coglet-r3-step-diagnostic/1",
            "arch": self.arch,
            "step": step,
            "status": status,
            "context": context,
            "pins": {
                "wasm_sha256": self.pins.wasm_sha256,
                "setup_sha256": self.pins.setup_sha256,
                "publication_sha256": self.pins.publication_sha256,
            },
            "outcome": result,
            "error": error,
        });
        let bytes = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
        let path = self.step_path(step);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        sync_dir(&self.dir)?;
        eprintln!("{}", summary(&self.arch, step, status, &doc));
        Ok(path)
    }

    /// Persist, panicking on any persistence failure: a run whose evidence
    /// cannot be retained must not pass.
    pub fn persist_or_panic<E: Debug>(
        &self,
        step: usize,
        context: &Value,
        outcome: &Result<Option<Value>, E>,
    ) -> PathBuf {
        self.persist(step, context, outcome)
            .unwrap_or_else(|e| panic!("r3 diagnostic persistence failed at step {step}: {e}"))
    }
}

fn summary(arch: &str, step: usize, status: &str, doc: &Value) -> String {
    let journal = &doc["outcome"]["journal"];
    let jstatus = journal["status"].as_str().unwrap_or("-");
    let failure = if journal["failure"].is_null() {
        doc["error"].to_string()
    } else {
        journal["failure"].to_string()
    };
    format!(
        "r3-diagnostic arch={arch} step={step} outcome={status} journal_status={jstatus} failure={failure}"
    )
}
