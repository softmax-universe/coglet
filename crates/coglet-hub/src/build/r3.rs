//! Protected local opt-in. No publication request can supply trust or executables.
use super::*;
use coglet_transform::{
    Allowlist, CiQualification, Evidence, GatePins, PublicationInputs, RawCiEvidence, TrustRoot,
};
use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;

/// Protected Recipe configuration. Never appears in the operator request schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct R3Config {
    pub trust: TrustRoot,
    pub run_id: u64,
    pub run_attempt: u64,
    /// Raw 32-byte admission issuer secret in a service-owned 0600 file.
    pub issuer_secret_file: PathBuf,
    pub issuer: String,
    /// Exact CI gate executable, containing its pinned interpreter build/config.
    pub engine_gate_sha256: String,
}
impl R3Config {
    pub(super) fn load(&self) -> Result<R3Profile> {
        #[cfg(not(target_os = "linux"))]
        {
            return Err(Error::Denied);
        }
        #[cfg(target_os = "linux")]
        {
            use std::{
                io::Read,
                os::unix::fs::{MetadataExt, OpenOptionsExt},
            };
            if !self.issuer_secret_file.is_absolute() {
                return Err(Error::Denied);
            }
            let file = io(std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&self.issuer_secret_file))?;
            let m = io(file.metadata())?;
            if !m.is_file()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o777 != 0o600
                || m.len() != 32
            {
                return Err(Error::Denied);
            }
            let mut bytes = Vec::new();
            io(file.take(33).read_to_end(&mut bytes))?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::Denied)?;
            let issuer = SigningKey::from_bytes(&bytes);
            let allowed: [u8; 32] = hex::decode(&self.issuer)
                .map_err(|_| Error::Denied)?
                .try_into()
                .map_err(|_| Error::Denied)?;
            let raw = RawCiEvidence::fetch(&self.trust, self.run_id, self.run_attempt)
                .map_err(|_| Error::Denied)?;
            let files = raw.verified_files(&self.trust).map_err(|_| Error::Denied)?;
            let engine = files
                .get("coglet-transform-gate")
                .ok_or(Error::Denied)?
                .clone();
            R3Profile::new(
                &raw,
                &self.trust,
                issuer,
                allowed,
                engine,
                &self.engine_gate_sha256,
            )
        }
    }
}

#[derive(Clone)]
pub struct R3Profile {
    qualification: CiQualification,
    files: BTreeMap<String, Vec<u8>>,
    raw: BTreeMap<String, Vec<u8>>,
    issuer: SigningKey,
    allowlist: Allowlist,
    engine_artifact: Vec<u8>,
}
impl R3Profile {
    /// All arguments are protected provisioning, NEVER request data. The engine
    /// digest must be independently approved for this exact native interpreter.
    /// CI identity is constructed exclusively by the verifier, including in tests.
    pub fn new(
        raw: &RawCiEvidence,
        trust: &TrustRoot,
        issuer: SigningKey,
        allowed_issuer: [u8; 32],
        engine_artifact: Vec<u8>,
        engine_sha256: &str,
    ) -> Result<Self> {
        let qualification =
            coglet_transform::verify_ci_qualification(raw, trust).map_err(|_| Error::Denied)?;
        let files = raw.verified_files(trust).map_err(|_| Error::Denied)?;
        let pin = trust
            .artifacts
            .get(&trust.target_artifact)
            .ok_or(Error::Denied)?;
        if !cfg!(target_os = "linux")
            || pin.arch != std::env::consts::ARCH
            || issuer.verifying_key().to_bytes() != allowed_issuer
            || engine_artifact.is_empty()
            || hash(&engine_artifact) != engine_sha256
            || files.get("coglet-transform-gate") != Some(&engine_artifact)
        {
            return Err(Error::Denied);
        }
        let get = |name: &str| files.get(name).map(|v| hash(v)).ok_or(Error::Denied);
        let pins = GatePins {
            profile: coglet_transform::PROFILE.into(),
            transform_artifact_sha256: qualification.transform_exe_sha256().into(),
            profile_bytes_sha256: hash(coglet_transform::profile_bytes()),
            engine: "wamr".into(),
            compute_mode: "wamr-2.4.3-fast-cumulative-v1".into(),
            arch: pin.arch.clone(),
            exec_mode: "native".into(),
            transform_source_sha256: get("transform.source")?,
            toolchain_sha256: get("rustc.txt")?,
            dependency_lock_sha256: get("Cargo.lock")?,
            recipe_sha256: hash(&recipe()),
            corpus_sha256: qualification.corpus_sha256().into(),
            gate_outputs_sha256: get("fuzz.results")?,
            engine_artifact_sha256: engine_sha256.into(),
        };
        Ok(Self {
            qualification,
            files,
            raw: raw.retained_blobs().clone(),
            issuer,
            allowlist: Allowlist {
                issuer: allowed_issuer,
                entries: vec![pins],
            },
            engine_artifact,
        })
    }
    pub(super) fn rewrite(&self, original: &[u8]) -> Result<Vec<u8>> {
        let source = self.run_tool("coglet-transform", &["--source"], &[], 4 * 1024 * 1024)?;
        if source != self.files["transform.source"]
            || !source.ends_with(coglet_transform::profile_bytes())
        {
            return Err(Error::Denied);
        }
        self.run_tool("coglet-transform", &[], original, MAX_WASM)
    }
    fn run_tool(
        &self,
        name: &str,
        args: &[&str],
        input_bytes: &[u8],
        max: usize,
    ) -> Result<Vec<u8>> {
        // Materialize ONLY authenticated bytes in a private directory. The child
        // gets no keys, ambient env, mutable request executable or network client.
        let dir = io(tempfile::tempdir())?;
        let exe = dir.path().join(name);
        // Never open the executable inode writable in this multithreaded
        // process: a concurrent fork can temporarily inherit even a CLOEXEC
        // writer and make exec fail ETXTBSY. A supervised fixed install child
        // owns and closes the only writer before we execute its distinct inode.
        let artifact = dir.path().join("artifact.bytes");
        io(std::fs::write(&artifact, &self.files[name]))?;
        let mut install = Command::new("/usr/bin/install");
        install
            .env_clear()
            .args([
                std::ffi::OsStr::new("-m"),
                std::ffi::OsStr::new("500"),
                std::ffi::OsStr::new("--"),
                artifact.as_os_str(),
                exe.as_os_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        process::run(install, 1024, Duration::from_secs(10))?;
        let input = dir.path().join("original.wasm");
        io(std::fs::write(&input, input_bytes))?;
        let mut command = Command::new(exe);
        command
            .args(args)
            .env_clear()
            .current_dir(dir.path())
            .stdin(Stdio::from(io(std::fs::File::open(input))?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        process::run(command, max, Duration::from_secs(10))
    }
    pub(super) fn evidence(&self, source: &[u8], original: &[u8], output: &[u8]) -> Result<Built> {
        let report = self.run_tool("coglet-transform-gate", &[], output, 16384)?;
        let parsed: Value = serde_json::from_slice(&report)?;
        if parsed["output_sha256"] != hash(output)
            || parsed["compute_mode"] != "wamr-2.4.3-fast-cumulative-v1"
            || parsed["cases"] != 2
        {
            return Err(Error::Denied);
        }
        let e = Evidence {
            publication_gate_outputs: report.clone(),
            pins: self.allowlist.entries[0].clone(),
            qualification: self.qualification.clone(),
            inputs: PublicationInputs {
                source_sha256: hash(source),
                original_sha256: hash(original),
                output_sha256: hash(output),
            },
            source: source.into(),
            transform_artifact: self.files["coglet-transform"].clone(),
            transform_source: self.files["transform.source"].clone(),
            profile: coglet_transform::profile_bytes().into(),
            toolchain: self.files["rustc.txt"].clone(),
            dependency_lock: self.files["Cargo.lock"].clone(),
            recipe: recipe(),
            corpus: self.files["fuzz.corpus"].clone(),
            gate_outputs: self.files["fuzz.results"].clone(),
            engine_artifact: self.engine_artifact.clone(),
            original: original.into(),
            output: output.into(),
        };
        e.verify().map_err(|_| Error::Denied)?;
        let mut blobs = self.raw.clone();
        blobs.insert("publication-gate.json".into(), report);
        for (name, bytes) in &self.files {
            blobs.insert(format!("ci/{name}"), bytes.clone());
        }
        for (name, bytes) in [
            ("source", source),
            ("original", original),
            ("rewritten", output),
            ("recipe", e.recipe.as_slice()),
            ("policy", e.profile.as_slice()),
            ("engine", e.engine_artifact.as_slice()),
        ] {
            blobs.insert(name.into(), bytes.into());
        }
        let q = &self.qualification;
        let index: BTreeMap<_, _> = blobs
            .iter()
            .map(|(name, bytes)| (name.clone(), hash(bytes)))
            .collect();
        let manifest = json!({"version":1,"evidence_sha256":e.digest(),"blobs":index,
            "ci":{"repo":q.repo(),"workflow_path":q.workflow_path(),"head_sha":q.head_sha(),"run_id":q.run_id(),"run_attempt":q.run_attempt(),"jobs":q.jobs(),"artifacts":q.artifacts()},
            "gates":{"output_sha256":hash(output),"abi":"0.1","compute_mode":"wamr-2.4.3-fast-cumulative-v1","exact_output":true}});
        Ok(Built {
            profile: self.clone(),
            evidence: e,
            blobs,
            manifest,
        })
    }
}
pub(super) struct Built {
    profile: R3Profile,
    pub(super) evidence: Evidence,
    pub(super) blobs: BTreeMap<String, Vec<u8>>,
    pub(super) manifest: Value,
}
impl Built {
    pub(super) fn issuer(&self) -> [u8; 32] {
        self.profile.allowlist.issuer
    }
    pub(super) fn admit(
        &self,
        candidate: &Candidate,
        sign: &SignRequest,
        provision: &Delegation,
        router: &iroh::SecretKey,
        tip: &str,
        now: u64,
    ) -> Result<Value> {
        use coglet_transform::{AdmissionStatement, Authority};
        let target = &candidate.target;
        let request = &candidate.request;
        let e = &self.evidence;
        let delegation = &provision.signed["statement"];
        let expiry = delegation["expires"].as_u64().ok_or(Error::Denied)?;
        let s = AdmissionStatement {
            publisher: router.public().to_string(),
            router: router.public().to_string(),
            coglet: target.coglet.clone(),
            runtime: target.device.clone(),
            source_commit: request.source_commit.clone(),
            original_sha256: e.inputs.original_sha256.clone(),
            output_sha256: e.inputs.output_sha256.clone(),
            profile: coglet_transform::PROFILE.into(),
            evidence_sha256: e.digest(),
            delegation_generation: delegation["generation"].as_u64().ok_or(Error::Denied)?,
            delegation_digest: hash(
                &coglet_proto::wire::encode(&provision.signed).map_err(|_| Error::Invalid)?,
            ),
            ref_old_tip: sign.old_tip.clone().unwrap_or_default(),
            ref_new_tip: tip.into(),
            ref_generation: sign.generation,
            valid_from_unix: now,
            expires_at_unix: expiry,
        };
        let authority = Authority {
            publisher: s.publisher.clone(),
            router: s.router.clone(),
            coglet: s.coglet.clone(),
            runtime: s.runtime.clone(),
            source_commit: s.source_commit.clone(),
            original_sha256: s.original_sha256.clone(),
            output_sha256: s.output_sha256.clone(),
            profile: s.profile.clone(),
            evidence_sha256: s.evidence_sha256.clone(),
            delegation_generation: s.delegation_generation,
            delegation_digest: s.delegation_digest.clone(),
            ref_old_tip: s.ref_old_tip.clone(),
            ref_new_tip: s.ref_new_tip.clone(),
            ref_generation: s.ref_generation,
            issuer: self.profile.allowlist.issuer,
            qualification: e.pins.clone(),
            ci_qualification: e.qualification.clone(),
            inputs: e.inputs.clone(),
            now_unix: now,
            delegation_expires_at_unix: expiry,
            revoked: false,
        };
        let token = coglet_transform::admit::sign(&self.profile.issuer, s);
        coglet_transform::admit_publication(&token, &self.profile.allowlist, &authority, e)
            .map_err(|_| Error::Denied)?;
        serde_json::to_value(token).map_err(Error::from)
    }
}
