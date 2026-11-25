//! Trusted-local publication builder. This is NOT an operator authorization or
//! promotion authority. Provision profiles from protected configuration, never requests.
//! The deliberately narrow integer-WAT policy admits only operator-reviewed exact
//! source, manifest and output pins. It does not qualify ADR-0003, floats, arbitrary
//! toolchains or M3 journal replay. Missing pins have no permissive fallback.
use crate::{Error, Result, Store};
use coglet_proto::{
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    transfer::DurableSink,
    validator::{Limits, Validator},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

mod process;
const MAX_INPUT: usize = 64 * 1024;
const MAX_WASM: usize = 64 * 1024;
const POLICY: &[u8] =
    b"coglet.integer-wat.exact-reviewed-output.v1;no-floats;abi-v0;no-rewrite;not-M3";
fn recipe() -> Vec<u8> {
    [
        &b"integer-WAT-v1;wat=1.263.0;wasmparser=0.245.1;guest.wat;manifest.json\n"[..],
        include_bytes!("../../../Cargo.lock"),
        &b"\nrouter-toolchain-lock\n"[..],
        include_bytes!("../../../rust-toolchain.toml"),
    ]
    .concat()
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn checked(rule: &str, value: &Value) -> Result<()> {
    let bytes = coglet_proto::wire::encode(value).map_err(|_| Error::Invalid)?;
    let value = coglet_types::decode_profile(&bytes).map_err(|_| Error::Invalid)?;
    coglet_types::validate(rule, &value).map_err(|_| Error::Invalid)
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|_| Error::Invalid)
}
/// Trusted provisioning. No request may supply a repository path or commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub repository: PathBuf,
    pub coglet: String,
    pub device: String,
}
/// An operator-approved finite artifact allowlist, not a caller's qualification
/// boolean. Hashes must come from independently reviewed artifacts. This policy
/// requires ALL three pins; a mere syntactically valid build is never enough.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    id: String,
    source_sha256: String,
    manifest_sha256: String,
    output_sha256: String,
}
impl Profile {
    pub fn integer_wat(
        id: &str,
        source_sha256: String,
        manifest_sha256: String,
        output_sha256: String,
    ) -> Self {
        Self {
            id: id.into(),
            source_sha256,
            manifest_sha256,
            output_sha256,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    pub source_commit: String,
    pub profile_id: String,
}
pub struct Builder {
    target: Target,
    profile: Profile,
}
/// Key-free compiled output; fields are private to prevent caller substitution.
pub struct Candidate {
    target: Target,
    profile: Profile,
    request: BuildRequest,
    source: Vec<u8>,
    manifest: Vec<u8>,
    wasm: Vec<u8>,
    provenance: Value,
}
impl Candidate {
    pub fn wasm(&self) -> &[u8] {
        &self.wasm
    }
    pub fn provenance(&self) -> &Value {
        &self.provenance
    }
}
impl Builder {
    pub fn new(target: Target, profile: Profile) -> Result<Self> {
        checked("coglet-id", &json!(target.coglet))?;
        checked("node-id", &json!(target.device))?;
        if !target.repository.is_absolute() || profile.id.is_empty() || profile.id.len() > 128 {
            return Err(Error::Invalid);
        }
        for pin in [
            &profile.source_sha256,
            &profile.manifest_sha256,
            &profile.output_sha256,
        ] {
            checked("hub-hash", &json!(pin))?;
        }
        Ok(Self { target, profile })
    }
    /// Bounded synchronous work: invoke on the service's blocking budget, never
    /// the async reactor. Reads pinned blobs only; no checkout, hooks, submodules,
    /// source build scripts, shell, ambient environment or coglet keys.
    pub fn compile(&self, request: &BuildRequest) -> Result<Candidate> {
        checked("hub-oid", &json!(request.source_commit))?;
        if request.profile_id != self.profile.id {
            return Err(Error::Denied);
        }
        let source = git(
            &self.target.repository,
            &["show", &format!("{}:guest.wat", request.source_commit)],
            None,
            MAX_INPUT,
        )?;
        let manifest = git(
            &self.target.repository,
            &["show", &format!("{}:manifest.json", request.source_commit)],
            None,
            MAX_INPUT,
        )?;
        if hash(&source) != self.profile.source_sha256
            || hash(&manifest) != self.profile.manifest_sha256
        {
            return Err(Error::Denied);
        }
        let parsed: Value = serde_json::from_slice(&manifest)?;
        // Keep router/provenance admission frozen, but publish only the current
        // ABI manifest. The old hub codec deliberately rejects ABI 1.0.
        let manifest_bytes = coglet_proto::wire::encode(&parsed).map_err(|_| Error::Invalid)?;
        coglet_types_current::decode("manifest", &manifest_bytes).map_err(|_| Error::Invalid)?;
        if parsed["id"] != self.target.coglet {
            return Err(Error::Denied);
        }
        let wasm = wat::parse_bytes(&source)
            .map_err(|_| Error::Invalid)?
            .into_owned();
        if wasm.len() > MAX_WASM {
            return Err(Error::Full);
        }
        let features = wasmparser::WasmFeatures::MUTABLE_GLOBAL
            | wasmparser::WasmFeatures::SIGN_EXTENSION
            | wasmparser::WasmFeatures::MULTI_VALUE
            | wasmparser::WasmFeatures::BULK_MEMORY
            | wasmparser::WasmFeatures::REFERENCE_TYPES;
        wasmparser::Validator::new_with_features(features)
            .validate_all(&wasm)
            .map_err(|_| Error::Denied)?;
        let limits = coglet_engine::Limits {
            compute_mode: "wamr.interpreter".into(),
            compute: 10000,
            memory_pages: parsed["budgets"]["memory_pages"]
                .as_u64()
                .ok_or(Error::Invalid)?
                .try_into()
                .map_err(|_| Error::Invalid)?,
            host_bytes: 65536,
            runtime_bytes: 16 * 1024 * 1024,
            stack_bytes: 64 * 1024,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        coglet_engine::admission::admit(&wasm, &limits).map_err(|_| Error::Denied)?;
        // The exact-output gate is pinned by the operator independently of this
        // compilation. No builder-produced success report can authorize itself.
        if hash(&wasm) != self.profile.output_sha256 {
            return Err(Error::Denied);
        }
        let provenance = json!({"version":"0.1","source_commit":request.source_commit,"recipe_sha256":hash(&recipe()),"policy_sha256":hash(POLICY),"transform_sha256":hash(include_bytes!("build.rs")),"original_sha256":hash(&wasm),"rewritten_sha256":hash(&wasm)});
        checked("hub-build-record", &provenance)?;
        Ok(Candidate {
            target: self.target.clone(),
            profile: self.profile.clone(),
            request: request.clone(),
            source,
            manifest,
            wasm,
            provenance,
        })
    }
}
/// Trusted provisioning resolves this identifier to a public signed delegation.
/// The coglet private key is deliberately not an argument to ANY build API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delegation {
    pub id: String,
    pub signed: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignRequest {
    pub uid: u32,
    pub operation_id: String,
    pub delegation_id: String,
    pub old_tip: Option<String>,
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    #[serde(skip)]
    pub bundle: Vec<u8>,
    pub signed: Value,
    pub provenance: Value,
    /// Exact inputs, including the original/output modules. Not audit payload.
    pub reproduction: Value,
}
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS build_artifacts(uid INTEGER NOT NULL,id TEXT NOT NULL,intent TEXT NOT NULL,metadata BLOB NOT NULL CHECK(length(metadata)<=1048576),bundle BLOB NOT NULL CHECK(length(bundle) BETWEEN 1 AND 67108864),bundle_sha256 TEXT NOT NULL,PRIMARY KEY(uid,id));
    CREATE TRIGGER IF NOT EXISTS build_artifacts_no_replace BEFORE INSERT ON build_artifacts WHEN EXISTS(SELECT 1 FROM build_artifacts WHERE uid=NEW.uid AND id=NEW.id) BEGIN SELECT RAISE(ABORT,'retained artifact'); END;
    CREATE TRIGGER IF NOT EXISTS build_artifacts_no_update BEFORE UPDATE ON build_artifacts BEGIN SELECT RAISE(ABORT,'immutable artifact'); END;
    CREATE TRIGGER IF NOT EXISTS build_artifacts_no_delete BEFORE DELETE ON build_artifacts BEGIN SELECT RAISE(ABORT,'retained artifact'); END;")?;
    Ok(())
}
impl Store {
    pub fn build_artifact(&self, uid: u32, id: &str) -> Result<Option<Artifact>> {
        checked("correlation-id", &json!(id))?;
        let row: Option<(Vec<u8>, Vec<u8>, String)> = self
            .db
            .query_row(
                "SELECT metadata,bundle,bundle_sha256 FROM build_artifacts WHERE uid=?1 AND id=?2",
                params![uid, id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        row.map(|(meta, bundle, digest)| {
            if meta.len() > 1024 * 1024
                || bundle.len() > 64 * 1024 * 1024
                || hash(&bundle) != digest
            {
                return Err(Error::Invalid);
            }
            let mut artifact: Artifact = serde_json::from_slice(&meta)?;
            artifact.bundle = bundle;
            Ok(artifact)
        })
        .transpose()
    }
    fn retained_build(&self, request: &SignRequest, intent: &str) -> Result<Option<Artifact>> {
        let old: Option<String> = self
            .db
            .query_row(
                "SELECT intent FROM build_artifacts WHERE uid=?1 AND id=?2",
                params![request.uid, request.operation_id],
                |r| r.get(0),
            )
            .optional()?;
        if old.as_ref().is_some_and(|old| old != intent) {
            return Err(Error::Conflict(0));
        }
        self.build_artifact(request.uid, &request.operation_id)
    }
    fn retain_build(
        &mut self,
        request: &SignRequest,
        intent: &str,
        artifact: Artifact,
    ) -> Result<Artifact> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT intent FROM build_artifacts WHERE uid=?1 AND id=?2",
                params![request.uid, request.operation_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            drop(tx);
            return if old == intent {
                self.build_artifact(request.uid, &request.operation_id)?
                    .ok_or(Error::Invalid)
            } else {
                Err(Error::Conflict(0))
            };
        }
        let metadata = serde_json::to_vec(&artifact)?;
        if metadata.len() > 1024 * 1024
            || artifact.bundle.is_empty()
            || artifact.bundle.len() > 64 * 1024 * 1024
        {
            return Err(Error::Full);
        }
        let (count, bytes): (i64, i64) = tx.query_row(
            "SELECT count(*),coalesce(sum(length(metadata)+length(bundle)),0) FROM build_artifacts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count < 0
            || bytes < 0
            || count >= 64
            || bytes + metadata.len() as i64 + artifact.bundle.len() as i64 > 128 * 1024 * 1024
        {
            return Err(Error::Full);
        }
        let digest = hash(&artifact.bundle);
        tx.execute(
            "INSERT INTO build_artifacts VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                request.uid,
                request.operation_id,
                intent,
                metadata,
                artifact.bundle,
                digest
            ],
        )?;
        crate::audit(
            &tx,
            json!({"method":"local.build.retain","uid":request.uid,"operation_id":request.operation_id,"bundle_sha256":digest,"commit":artifact.signed["statement"]["new"]}),
        )?;
        tx.commit()?;
        Ok(artifact)
    }
}
fn signed(key: &iroh::SecretKey, statement: Value) -> Result<Value> {
    let sig = key
        .sign(&coglet_proto::signing::preimage("hub-ref", &statement).map_err(|_| Error::Invalid)?);
    let value = json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(sig.to_bytes())});
    coglet_proto::signing::verify(&value, "hub-ref", &key.public().to_string())
        .map_err(|_| Error::Invalid)?;
    Ok(value)
}
impl Candidate {
    fn delegation(
        &self,
        provision: &Delegation,
        key: &iroh::SecretKey,
        now: Option<u64>,
    ) -> Result<()> {
        let now = now.ok_or(Error::Denied)?;
        coglet_proto::signing::verify(&provision.signed, "delegation", &self.target.coglet[7..])
            .map_err(|_| Error::Denied)?;
        let s = &provision.signed["statement"];
        if s["hub"] != key.public().to_string()
            || s["device"] != self.target.device
            || s["coglet"] != self.target.coglet
            || !s["expires"].as_u64().is_some_and(|expires| now < expires)
            || !s["methods"]
                .as_array()
                .is_some_and(|methods| methods.contains(&json!("hub.git.push")))
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    /// Return only after exact bundle/metadata/inputs and audit commit together.
    /// This is publication, NOT dispatch authorization: operator, enrollment,
    /// delegation revocation, promotion gates, ref CAS and kill switch must still
    /// be rechecked by the saga immediately before any new network effect.
    /// Bounded blocking API. Async services must invoke this under their blocking
    /// permit. The private async validator runs on this dedicated worker runtime.
    #[allow(clippy::too_many_arguments)]
    pub fn sign_and_retain(
        self,
        store: &mut Store,
        request: &SignRequest,
        provision: &Delegation,
        key: &iroh::SecretKey,
        authority: &coglet_proto::authority::Authority,
        validator: &Validator,
        quarantine: &Path,
        clock: impl Fn() -> Option<u64>,
    ) -> Result<Artifact> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::Invalid)?;
        runtime.block_on(self.sign_and_retain_inner(
            store, request, provision, key, authority, validator, quarantine, clock,
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn sign_and_retain_inner(
        self,
        store: &mut Store,
        request: &SignRequest,
        provision: &Delegation,
        key: &iroh::SecretKey,
        authority: &coglet_proto::authority::Authority,
        validator: &Validator,
        quarantine: &Path,
        clock: impl Fn() -> Option<u64>,
    ) -> Result<Artifact> {
        checked("correlation-id", &json!(request.operation_id))?;
        if request.delegation_id != provision.id || provision.id.is_empty() {
            return Err(Error::Denied);
        }
        if let Some(old) = &request.old_tip {
            checked("hub-oid", &json!(old))?;
        }
        let intent = hash(&serde_json::to_vec(
            &json!({"target":self.target,"profile":self.profile,"build":self.request,"sign":request,"delegation":provision,"hub":key.public().to_string(),"recipe":hash(&recipe()),"policy":hash(POLICY),"transform":self.provenance["transform_sha256"]}),
        )?);
        if let Some(artifact) = store.retained_build(request, &intent)? {
            return Ok(artifact);
        }
        self.delegation(provision, key, clock())?;
        authority.authorize_delegation(
            key.public(),
            self.target.device.parse().map_err(|_| Error::Invalid)?,
            &self.target.coglet,
            "hub.git.push",
            &clock,
            provision.signed["statement"]["generation"]
                .as_u64()
                .ok_or(Error::Denied)?,
        )?;
        let stage = io(tempfile::tempdir_in(quarantine))?;
        for dir in ["source", "bin", "build"] {
            io(std::fs::create_dir(stage.path().join(dir)))?;
        }
        let recipe = recipe();
        for (name, bytes) in [
            ("source/guest.wat", self.source.as_slice()),
            ("manifest.json", self.manifest.as_slice()),
            ("bin/step.wasm", self.wasm.as_slice()),
            ("build/recipe.lock", recipe.as_slice()),
            ("build/policy.txt", POLICY),
        ] {
            io(std::fs::write(stage.path().join(name), bytes))?;
        }
        io(std::fs::write(
            stage.path().join("build/provenance.cbor"),
            coglet_proto::wire::encode(&self.provenance).map_err(|_| Error::Invalid)?,
        ))?;
        git(stage.path(), &["init", "--initial-branch=hub"], None, 1024)?;
        git(
            stage.path(),
            &["add", "source", "manifest.json", "bin", "build"],
            None,
            1024,
        )?;
        git(
            stage.path(),
            &["commit", "-m", "qualified integer-WAT publication"],
            None,
            1024,
        )?;
        let tip = String::from_utf8(git(stage.path(), &["rev-parse", "HEAD"], None, 1024)?)
            .map_err(|_| Error::Invalid)?
            .trim()
            .to_owned();
        let archive = git(
            stage.path(),
            &["archive", "--format=tar", "HEAD"],
            None,
            64 * 1024 * 1024,
        )?;
        git(
            stage.path(),
            &["bundle", "create", "transfer.bundle", "refs/heads/hub"],
            None,
            1024,
        )?;
        let bundle = io(std::fs::read(stage.path().join("transfer.bundle")))?;
        if bundle.len() > 64 * 1024 * 1024 {
            return Err(Error::Full);
        }
        // Frozen validator requires signed metadata. Validate under a disposable
        // NON-router identity first; the qualified router key is used only after
        // validation. A temporary signature cannot grant router publication.
        let validation_key = iroh::SecretKey::generate();
        let mut statement = json!({"purpose":"hub-ref","hub":validation_key.public().to_string(),"coglet":self.target.coglet,"ref":"refs/heads/hub","old":request.old_tip,"new":tip,"archive_sha256":hash(&archive),"generation":request.generation});
        let preflight = signed(&validation_key, statement.clone())?;
        let metadata = VerifiedRef::verify(
            &preflight,
            RefKind::Hub,
            &validation_key.public().to_string(),
            &self.target.coglet,
        )
        .map_err(|_| Error::Invalid)?;
        let mut sink = io(BundleSink::create(quarantine, bundle.len() as u64).await)?;
        io(sink.append(&bundle).await)?;
        let envelope = io(sink.check_envelope(&metadata, &hash(&bundle), 100000).await)?;
        let verified = envelope
            .validate(validator, Limits::default())
            .await
            .map_err(|_| Error::Denied)?;
        if verified.metadata().new_tip() != tip {
            return Err(Error::Invalid);
        }
        drop(verified);
        self.delegation(provision, key, clock())?;
        authority.authorize_delegation(
            key.public(),
            self.target.device.parse().map_err(|_| Error::Invalid)?,
            &self.target.coglet,
            "hub.git.push",
            &clock,
            provision.signed["statement"]["generation"]
                .as_u64()
                .ok_or(Error::Denied)?,
        )?;
        statement["hub"] = json!(key.public().to_string());
        let signature = signed(key, statement)?;
        let artifact = Artifact {
            bundle,
            signed: signature,
            provenance: self.provenance.clone(),
            reproduction: json!({"source":hex::encode(&self.source),"manifest":hex::encode(&self.manifest),"recipe":hex::encode(&recipe),"policy":hex::encode(POLICY),"transform":hex::encode(include_bytes!("build.rs")),"original":hex::encode(&self.wasm),"rewritten":hex::encode(&self.wasm),"delegation":provision,"profile":self.profile,"target":self.target,"request":self.request}),
        };
        store.retain_build(request, &intent, artifact)
    }
}
/// Fixed Git executable, clean configuration and bounded output. No shell and no
/// repository-selected executable. Source/output limits precede all allocations.
fn git(root: &Path, args: &[&str], input: Option<&[u8]>, max: usize) -> Result<Vec<u8>> {
    let mut cmd = Command::new("/usr/bin/git");
    cmd.current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        .env("GIT_CONFIG_KEY_1", "protocol.allow")
        .env("GIT_CONFIG_VALUE_1", "never")
        .env("GIT_AUTHOR_NAME", "Coglet router")
        .env("GIT_AUTHOR_EMAIL", "router@example.invalid")
        .env("GIT_COMMITTER_NAME", "Coglet router")
        .env("GIT_COMMITTER_EMAIL", "router@example.invalid")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if input.is_some() {
        return Err(Error::Invalid);
    }
    cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_TEMPLATE_DIR", "");
    process::run(cmd, max, Duration::from_secs(10))
}
